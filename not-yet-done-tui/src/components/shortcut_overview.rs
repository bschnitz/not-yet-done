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
//! A long list goes **wide before it goes long**: when the sections do not fit
//! the terminal's height they are laid out in columns, as many as the width
//! allows, and only what still overflows scrolls. A section too tall for one
//! column spreads over several and takes a row band of its own; the short ones
//! sit side by side in the next band, headings aligned, and once that band is
//! served they stack up in the room left beside its tallest section instead of
//! opening another one. Reading beats scrolling — the whole point of the popup
//! is to see the context at once.
//!
//! What is left to scroll takes `j`/`k` (and the arrows), `ctrl+d`/`ctrl+u`
//! and the page keys for a screen, `g`/`G` for the ends; any other key closes
//! it. A column is content-sized between [`ShortcutOverviewConfig::min_width`]
//! and [`ShortcutOverviewConfig::max_width`] where those are configured — the
//! bounds size one column, not the whole popup, so long names are truncated at
//! the maximum instead of stretching every column across the terminal.
//!
//! [shortcut menu]: crate::components::shortcut_menu::ShortcutMenu
//! [`GlobalAction::ShortcutOverview`]: crate::config::keybindings::GlobalAction::ShortcutOverview
//! [`WhichKeyGroup`]: crate::config::tui_config::WhichKeyGroup
//! [`ShortcutOverviewConfig::min_width`]: crate::config::tui_config::ShortcutOverviewConfig::min_width
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

/// Blank cells between two columns.
const COL_GAP: usize = 4;

/// Entries sit under their heading, so the sections read as blocks.
const INDENT: &str = "  ";

/// One rendered line of the overview.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Row {
    /// A section heading: the group's title (or its prefix), plus the prefix
    /// itself when the title does not already say it.
    Heading { title: String, prefix: String },
    /// One shortcut: name on the left, keys on the right.
    Entry { name: String, keys: String },
}

/// A heading and the entries under it — the unit the column layout moves
/// around. Kept as blocks rather than one flat list because a column break
/// must know where a section starts and how tall it is.
type Section = Vec<Row>;

/// The read-only, grouped shortcut list.
pub struct ShortcutOverview {
    theme: Arc<Theme>,
    open: bool,
    /// Narrowest and widest a *column* may get, from
    /// `shortcut_overview.min_width` / `.max_width`. Both unset by default:
    /// a column is then sized by its content alone.
    min_width: Option<u16>,
    max_width: Option<u16>,
    /// The sections, in display order.
    sections: Vec<Section>,
    /// First line shown — the scroll offset.
    offset: usize,
    /// How many lines the last render fit, for page scrolling and clamping.
    viewport: usize,
    /// How many lines the last layout produced. Not the row count: columns
    /// make the list shorter than the sum of its sections.
    grid_height: usize,
}

impl ShortcutOverview {
    pub fn new(theme: Arc<Theme>, min_width: Option<u16>, max_width: Option<u16>) -> Self {
        Self {
            theme,
            open: false,
            min_width,
            max_width,
            sections: Vec::new(),
            offset: 0,
            viewport: 1,
            grid_height: 0,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open the overview for `rows`, grouped by `groups`. Rows without a key
    /// (actions waiting for a binding — the menu's "unbound" view) are left
    /// out: there is nothing to look up about them here.
    pub fn open(&mut self, rows: Vec<ShortcutRow>, groups: &[WhichKeyGroup]) {
        self.sections = build_sections(rows, groups);
        self.offset = 0;
        // Until the first render decides on a column count, the list is as
        // tall as it would be in a single column.
        self.grid_height = self.sections.iter().map(Vec::len).sum::<usize>()
            + self.sections.len().saturating_sub(1);
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
        self.grid_height.saturating_sub(self.viewport)
    }

    fn scroll(&mut self, delta: isize) {
        let next = self.offset as isize + delta;
        self.offset = next.clamp(0, self.max_offset() as isize) as usize;
    }

    /// Draw the panel and the visible lines of the grid.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        if !self.open || self.sections.is_empty() {
            return;
        }
        let theme = Arc::clone(&self.theme);
        let hints = vec![("j/k", "scroll"), ("g/G", "top/bottom"), ("esc", "close")];
        let col_w = self.column_width();

        // The body's shape depends on the room it gets, so the room is asked
        // for first — at the width of a single column, which is the narrowest
        // the panel can end up and therefore the safe estimate.
        let (avail_w, avail_h) = PanelChrome::new(heading(&theme))
            .hints(hints.clone())
            .body(col_w, 0)
            .available(area);
        // Borrowing the sections rather than `self` so the offset bookkeeping
        // below can still write to the other fields.
        let (grid, cols) = fit_columns(&self.sections, col_w, avail_w as usize, avail_h as usize);
        let grid_h = grid.first().map(Vec::len).unwrap_or(0);
        let grid_w = cols * col_w + (cols - 1) * COL_GAP;

        let body = PanelChrome::new(heading(&theme))
            .hints(hints)
            .body(grid_w, grid_h as u16)
            .render(frame, area, &theme);
        let Some(body) = body else { return };

        // The viewport is only known once the chrome has laid the panel out,
        // so the offset is clamped here rather than in `handle_key` — a
        // resize that shrinks the popup must not leave it scrolled past its
        // end.
        self.viewport = body.height as usize;
        self.grid_height = grid_h;
        self.offset = self.offset.min(self.max_offset());

        let visible = (body.height as usize).min(grid_h.saturating_sub(self.offset));
        for i in 0..visible {
            let line = compose_line(&grid, self.offset + i, col_w, &theme);
            frame.render_widget(
                Paragraph::new(line),
                Rect::new(body.x, body.y + i as u16, body.width, 1),
            );
        }
    }

    /// The width of one column: the widest line, then the configured bounds.
    /// `min_width` is applied last, so a minimum wider than the maximum still
    /// holds rather than leaving the two to fight.
    fn column_width(&self) -> usize {
        let widest = self
            .sections
            .iter()
            .flatten()
            .map(|row| match row {
                Row::Heading { title, prefix } => heading_text(title, prefix).chars().count(),
                Row::Entry { name, keys } => {
                    INDENT.len() + name.chars().count() + GAP + keys.chars().count()
                }
            })
            .max()
            .unwrap_or(0);
        let capped = match self.max_width {
            Some(max) => widest.min(max as usize),
            None => widest,
        };
        match self.min_width {
            Some(min) => capped.max(min as usize),
            None => capped,
        }
    }
}

/// Lay the sections out in as few columns as fit the height, and no more than
/// fit the width. One column is the normal case; a second is added only once
/// the list would otherwise be cut off, so a short list keeps the compact
/// popup it has always had. What does not fit even then still scrolls.
fn fit_columns<'a>(
    sections: &'a [Section],
    col_w: usize,
    avail_w: usize,
    avail_h: usize,
) -> (Vec<Vec<Option<&'a Row>>>, usize) {
    let max_cols = ((avail_w + COL_GAP) / (col_w + COL_GAP)).max(1);
    let mut cols = 1;
    loop {
        let grid = layout(sections, cols, avail_h);
        let fits = grid.first().map(Vec::len).unwrap_or(0) <= avail_h;
        if fits || cols >= max_cols {
            return (grid, cols);
        }
        cols += 1;
    }
}

/// Pack `sections` into `cols` columns of `col_h` lines each, and pad them to
/// a common height so a line index addresses the same row in every column.
///
/// The unit of placement is the section, and sections are laid out in **row
/// bands**: the first section a column takes in a band starts on the band's
/// top line, so headings line up across the popup instead of sitting at
/// arbitrary heights. A section too tall for one column opens a band of its
/// own and is spread over as many columns as it needs, balanced so the last
/// one is not left almost empty.
///
/// Once every column of a band has been served, the sections that follow
/// **stack up inside the band** rather than opening a new one below it — that
/// is the empty space beside a tall section, and a long "General" next to a
/// handful of short groups would otherwise waste most of the popup. Only when
/// a section no longer fits inside the band does a new band start. With
/// `cols == 1` this degrades exactly to the single-column list: sections
/// stacked, one blank line between them.
fn layout<'a>(sections: &'a [Section], cols: usize, col_h: usize) -> Vec<Vec<Option<&'a Row>>> {
    let cols = cols.max(1);
    let col_h = col_h.max(1);
    let mut columns: Vec<Vec<Option<&Row>>> = vec![Vec::new(); cols];
    // Where the current band starts. A column still at or above this line has
    // not been served in this band yet.
    let mut band_top = 0usize;

    for section in sections {
        let spans = section.len().div_ceil(col_h).min(cols).max(1);
        if spans > 1 {
            band_top = next_band(&columns);
            let per = section.len().div_ceil(spans);
            for (i, chunk) in section.chunks(per).enumerate() {
                place(&mut columns[i], band_top, chunk);
            }
            continue;
        }
        // A column the band has not touched yet, left to right — this is what
        // aligns the headings.
        if let Some(col) = columns.iter().position(|c| c.len() <= band_top) {
            place(&mut columns[col], band_top, section);
            continue;
        }
        // None left: stack into the column with the most room, as long as the
        // section stays within the band. One blank line of air above it.
        let band_bottom = columns.iter().map(Vec::len).max().unwrap_or(0);
        let roomiest = columns
            .iter()
            .enumerate()
            .min_by_key(|(_, c)| c.len())
            .map(|(i, _)| i)
            .unwrap_or(0);
        let top = columns[roomiest].len() + 1;
        if top + section.len() <= band_bottom {
            place(&mut columns[roomiest], top, section);
            continue;
        }
        band_top = next_band(&columns);
        place(&mut columns[0], band_top, section);
    }

    let height = columns.iter().map(Vec::len).max().unwrap_or(0);
    for column in &mut columns {
        column.resize(height, None);
    }
    columns
}

/// The line the next band starts on: below everything placed so far, with one
/// blank line of air (none at the very top).
fn next_band(columns: &[Vec<Option<&Row>>]) -> usize {
    match columns.iter().map(Vec::len).max().unwrap_or(0) {
        0 => 0,
        used => used + 1,
    }
}

/// Put `rows` into `column` starting at line `top`, filling the gap above
/// with blanks.
fn place<'a>(column: &mut Vec<Option<&'a Row>>, top: usize, rows: &'a [Row]) {
    while column.len() < top {
        column.push(None);
    }
    column.extend(rows.iter().map(Some));
}

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

/// Sort `rows` into sections. "General" comes first and holds everything no
/// group claims; the groups follow in config order, each with the entries
/// bound under its prefix. Empty sections are dropped.
fn build_sections(rows: Vec<ShortcutRow>, groups: &[WhichKeyGroup]) -> Vec<Section> {
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

    sections
        .into_iter()
        .filter(|(_, _, entries)| !entries.is_empty())
        .map(|(title, prefix, entries)| {
            let mut section = vec![Row::Heading { title, prefix }];
            section.extend(entries);
            section
        })
        .collect()
}

/// One full line of the grid: every column's cell at `idx`, separated by
/// [`COL_GAP`] blanks. Cells are padded to `col_w`, so the columns stay
/// aligned whatever sits in them.
fn compose_line(
    grid: &[Vec<Option<&Row>>],
    idx: usize,
    col_w: usize,
    theme: &Theme,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, column) in grid.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" ".repeat(COL_GAP)));
        }
        match column.get(idx).copied().flatten() {
            Some(row) => spans.extend(row_spans(row, col_w, theme)),
            None => spans.push(Span::raw(" ".repeat(col_w))),
        }
    }
    Line::from(spans)
}

/// One cell, exactly `width` cells wide.
fn row_spans(row: &Row, width: usize, theme: &Theme) -> Vec<Span<'static>> {
    match row {
        Row::Heading { title, prefix } => {
            let text = truncate(&heading_text(title, prefix), width);
            let pad = width.saturating_sub(text.chars().count());
            vec![
                Span::styled(
                    text,
                    Style::default()
                        .fg(theme.form_accent())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" ".repeat(pad)),
            ]
        }
        Row::Entry { name, keys } => {
            let keys_w = keys.chars().count();
            // Keys are the part worth reading; the name yields when the two
            // cannot both fit.
            let room = width
                .saturating_sub(INDENT.len())
                .saturating_sub(keys_w + GAP);
            let name = truncate(name, room);
            let fill = room.saturating_sub(name.chars().count()) + GAP;
            vec![
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
            ]
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
        let mut o = ShortcutOverview::new(Arc::new(Theme::new(ThemeConfig::default())), None, None);
        o.open(rows, groups);
        o
    }

    fn heading_row(title: &str, prefix: &str) -> Row {
        Row::Heading {
            title: title.to_string(),
            prefix: prefix.to_string(),
        }
    }

    fn entry(name: &str, keys: &str) -> Row {
        Row::Entry {
            name: name.to_string(),
            keys: keys.to_string(),
        }
    }

    /// The heading of the section a cell belongs to is not recoverable from
    /// the cell itself, so tests read the grid as text.
    fn cell_text(cell: Option<&Row>) -> String {
        match cell {
            None => String::new(),
            Some(Row::Heading { title, prefix }) => heading_text(title, prefix),
            Some(Row::Entry { name, .. }) => name.clone(),
        }
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
            o.sections,
            vec![
                vec![heading_row(GENERAL, ""), entry("edit", "e")],
                vec![
                    heading_row("Open ...", "o"),
                    entry("open in browser", "o b")
                ],
                vec![heading_row("q", "q"), entry("queries", "q q")],
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
            o.sections,
            vec![vec![heading_row(GENERAL, ""), entry("notes", "o")]]
        );
    }

    #[test]
    fn an_empty_group_gets_no_heading() {
        let groups = vec![group("o", Some("Open ...")), group("z", Some("Fold ..."))];
        let o = overview(vec![row("open in browser", "o b")], &groups);
        assert!(!o.sections.iter().flatten().any(|r| matches!(
            r,
            Row::Heading { title, .. } if title == "Fold ..."
        )));
    }

    #[test]
    fn the_same_shortcut_is_listed_once() {
        let o = overview(vec![row("quit", "ctrl+c"), row("quit", "ctrl+c")], &[]);
        assert_eq!(
            o.sections
                .iter()
                .flatten()
                .filter(|r| matches!(r, Row::Entry { .. }))
                .count(),
            1
        );
    }

    /// A list that fits keeps the single column it always had — no gratuitous
    /// second column, and the sections still read top to bottom with a blank
    /// line between them.
    #[test]
    fn a_list_that_fits_stays_one_column() {
        let groups = vec![group("o", Some("Open ..."))];
        let o = overview(
            vec![row("edit", "e"), row("open in browser", "o b")],
            &groups,
        );
        let (grid, cols) = fit_columns(&o.sections, 40, 200, 40);
        assert_eq!(cols, 1);
        let texts: Vec<String> = grid[0].iter().map(|c| cell_text(*c)).collect();
        assert_eq!(
            texts,
            vec![
                "General".to_string(),
                "edit".to_string(),
                String::new(),
                "Open ...  (o)".to_string(),
                "open in browser".to_string(),
            ]
        );
    }

    /// Too tall for the terminal: a second column is added rather than
    /// scrolling, and a section that alone outgrows a column is spread over
    /// the columns it needs.
    #[test]
    fn a_tall_section_spreads_over_columns_instead_of_scrolling() {
        let rows: Vec<ShortcutRow> = (0..30).map(|i| row(&format!("action {i}"), "a")).collect();
        let o = overview(rows, &[]);
        // 31 lines (heading + 30) into columns of 20.
        let (grid, cols) = fit_columns(&o.sections, 40, 200, 20);
        assert_eq!(cols, 2, "one more column, not a scrollbar");
        assert!(
            grid[0].len() <= 20,
            "the column must fit the height: {}",
            grid[0].len()
        );
        // Balanced, so the second column is not left nearly empty.
        let filled = |c: &Vec<Option<&Row>>| c.iter().filter(|cell| cell.is_some()).count();
        assert_eq!(filled(&grid[0]) + filled(&grid[1]), 31);
        assert!(filled(&grid[1]) * 2 >= filled(&grid[0]));
        // The heading stays with the first chunk; the split is a plain
        // continuation, not a repeated heading.
        assert_eq!(cell_text(grid[0][0]), "General");
        assert!(
            !grid[1]
                .iter()
                .any(|c| matches!(c, Some(Row::Heading { .. })))
        );
    }

    /// Short sections that follow a spread-out one share the next band, so
    /// their headings sit on the same line.
    #[test]
    fn short_sections_share_a_band_with_aligned_headings() {
        let mut rows: Vec<ShortcutRow> =
            (0..30).map(|i| row(&format!("action {i}"), "a")).collect();
        rows.push(row("open in browser", "o b"));
        rows.push(row("open preview", "o p"));
        rows.push(row("blub one", "b b"));
        let groups = vec![group("o", Some("Open ...")), group("b", Some("Blub ..."))];
        let o = overview(rows, &groups);

        let (grid, cols) = fit_columns(&o.sections, 40, 200, 20);
        assert_eq!(cols, 2);
        let band = grid[0]
            .iter()
            .position(|c| cell_text(*c) == "Open ...  (o)")
            .expect("Open section placed");
        assert_eq!(
            cell_text(grid[1][band]),
            "Blub ...  (b)",
            "the next section sits beside it on the same line"
        );
    }

    /// The space beside a tall section is filled: once every column of the
    /// band has a section, the ones that follow stack up in the roomiest
    /// column instead of opening a band below the tall one.
    #[test]
    fn short_sections_stack_beside_a_tall_one() {
        let mut rows: Vec<ShortcutRow> =
            (0..30).map(|i| row(&format!("action {i}"), "a")).collect();
        rows.push(row("open in browser", "o b"));
        rows.push(row("blub one", "b b"));
        let groups = vec![group("o", Some("Open ...")), group("b", Some("Blub ..."))];
        let o = overview(rows, &groups);

        // General is 31 lines and fills the first column; both short sections
        // fit next to it in the second.
        let (grid, cols) = fit_columns(&o.sections, 40, 200, 35);
        assert_eq!(cols, 2);
        assert_eq!(cell_text(grid[1][0]), "Open ...  (o)");
        assert_eq!(
            cell_text(grid[1][3]),
            "Blub ...  (b)",
            "the second short section sits under the first, not in a new band"
        );
        assert_eq!(
            grid[0].len(),
            31,
            "the grid stays as tall as the tall section"
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
        assert_eq!(o.offset, o.grid_height - 5);
        o.handle_key("j");
        assert_eq!(
            o.offset,
            o.grid_height - 5,
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

    /// Unbounded by default (a column is then content-sized like every other
    /// popup); `max_width` cuts a long list down, `min_width` pads a short one
    /// out, and a minimum wider than the maximum still wins.
    #[test]
    fn the_width_bounds_are_optional_and_the_minimum_has_the_last_word() {
        let long = "n".repeat(200);
        let mut o = overview(vec![row(&long, "ctrl+alt+shift+f12")], &[]);
        let natural = o.column_width();
        assert!(natural > 200, "unbounded: the widest line wins");

        o.max_width = Some(20);
        assert_eq!(o.column_width(), 20);

        o.max_width = None;
        o.min_width = Some(50);
        assert_eq!(o.column_width(), natural, "a wide list ignores the minimum");

        let mut short = overview(vec![row("edit", "e")], &[]);
        assert!(short.column_width() < 50);
        short.min_width = Some(50);
        assert_eq!(short.column_width(), 50);

        short.max_width = Some(20);
        assert_eq!(
            short.column_width(),
            50,
            "the minimum wins over a lower cap"
        );
    }

    /// Columns only go as wide as the terminal allows; what is left over
    /// scrolls as before.
    #[test]
    fn the_width_caps_the_column_count() {
        let rows: Vec<ShortcutRow> = (0..100).map(|i| row(&format!("action {i}"), "a")).collect();
        let o = overview(rows, &[]);
        let (grid, cols) = fit_columns(&o.sections, 40, 90, 10);
        assert_eq!(cols, 2, "only two columns of 40 fit in 90 cells");
        assert!(
            grid[0].len() > 10,
            "the rest still scrolls: {}",
            grid[0].len()
        );
    }

    #[test]
    fn a_row_renders_name_dots_keys_within_the_width() {
        let theme = Theme::new(ThemeConfig::default());
        let spans = row_spans(&entry("open in browser", "o b"), 30, &theme);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text.chars().count(), 30);
        assert!(text.starts_with("  open in browser "));
        assert!(text.ends_with(" o b"));
    }

    #[test]
    fn a_name_too_long_for_the_width_is_cut_not_the_keys() {
        let theme = Theme::new(ThemeConfig::default());
        let spans = row_spans(
            &entry("an extremely long shortcut name", "ctrl+x"),
            20,
            &theme,
        );
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text.chars().count(), 20);
        assert!(text.contains('\u{2026}'), "the name is elided: {text}");
        assert!(text.ends_with("ctrl+x"));
    }

    /// Every cell is padded to the column width, so the columns stay aligned
    /// however ragged the sections are.
    #[test]
    fn every_line_of_the_grid_is_the_same_width() {
        let theme = Theme::new(ThemeConfig::default());
        let rows: Vec<ShortcutRow> = (0..30).map(|i| row(&format!("action {i}"), "a")).collect();
        let o = overview(rows, &[]);
        let (grid, cols) = fit_columns(&o.sections, 30, 200, 20);
        let expected = cols * 30 + (cols - 1) * COL_GAP;
        for idx in 0..grid[0].len() {
            let line = compose_line(&grid, idx, 30, &theme);
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            assert_eq!(text.chars().count(), expected, "line {idx}: {text:?}");
        }
    }
}
