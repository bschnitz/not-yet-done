//! The notification centre — every message both bars have shown, on one page,
//! with copy.
//!
//! Opened by [`GlobalAction::ShowNotifications`] (default `f10`). The bars
//! themselves are a glance, not a record: the bottom one shows the last
//! message or two and `Z` wipes it, while an error worth reporting is usually
//! wanted *after* it has scrolled away — with its wording intact, in a
//! bug report or a chat message. That is what this page is for.
//!
//! It lists both bars' logs merged chronologically, newest first, each entry
//! stamped and marked by what it is (`✖` error, `▲` alert, `●` message). The
//! entry under the cursor is expanded in place, wrapped over the full width,
//! so a stack trace or a long adapter error can be read without leaving the
//! app. `y` copies that one message, `Y` copies the whole list as a
//! timestamped log, `e` narrows the page to errors, and `o` still hands the
//! log to `$EDITOR` for the cases only a real editor solves (search, huge
//! payloads, saving it somewhere).
//!
//! [`GlobalAction::ShowNotifications`]: crate::config::keybindings::GlobalAction::ShowNotifications

use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::components::notification_bar::{NoticeLevel, NotificationRecord};
use crate::ui::panel_chrome::{PanelChrome, cursor_bg, pad_row};
use crate::ui::theme::Theme;

/// Cells the stamp column takes: the cursor gutter, `HH:MM:SS`, two spaces,
/// the marker glyph and the space after it. Continuation lines are indented to
/// the same column so a wrapped message reads as one block.
const TEXT_COL: usize = 14;

/// The two cells in front of every row that carry the cursor bar. A cursor
/// row is filled behind, but a background alone is easy to miss on a theme
/// whose `form_field_bg` sits close to the panel — the bar never is.
const GUTTER: &str = "\u{258d} ";

/// Cells kept free on the right for the scroll indicator.
const BAR_COL: usize = 2;

/// Narrowest and widest the page gets. A notification log is mostly prose,
/// and prose past ~100 cells is harder to read, not easier.
const MIN_WIDTH: usize = 56;
const MAX_WIDTH: usize = 100;

/// One logged message, as the page shows it.
#[derive(Debug, Clone)]
pub struct Entry {
    pub at: chrono::DateTime<chrono::Local>,
    pub message: String,
    pub level: NoticeLevel,
    /// Pushed by the prominent top bar rather than the bottom one.
    pub alert: bool,
}

impl Entry {
    /// The entries of one bar, tagged with whether that bar is the alert one.
    pub fn from_records(records: &[NotificationRecord], alert: bool) -> Vec<Self> {
        records
            .iter()
            .map(|r| Entry {
                at: r.at,
                message: r.message.clone(),
                level: r.level,
                alert,
            })
            .collect()
    }

    fn is_error(&self) -> bool {
        self.level == NoticeLevel::Error
    }
}

/// What a key press means for the embedder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CenterOutcome {
    /// Handled inside the page; nothing for the App to do.
    Consumed,
    /// Close the page.
    Close,
    /// Close the page and hand the log to `$EDITOR`.
    OpenEditor,
}

/// The last copy attempt, shown until the next key press. Copy feedback stays
/// *inside* the page on purpose: routing it through the notification bar would
/// log "copied a message" into the very log being copied.
struct Flash {
    ok: bool,
    text: String,
}

/// The scrollable, copyable notification log.
pub struct NotificationCenter {
    theme: Arc<Theme>,
    open: bool,
    /// All entries, newest first.
    entries: Vec<Entry>,
    /// Cursor into the *filtered* list.
    cursor: usize,
    /// First visible rendered line.
    offset: usize,
    /// Rendered lines the last render fit, for page keys and clamping.
    viewport: usize,
    errors_only: bool,
    flash: Option<Flash>,
}

impl NotificationCenter {
    pub fn new(theme: Arc<Theme>) -> Self {
        Self {
            theme,
            open: false,
            entries: Vec::new(),
            cursor: 0,
            offset: 0,
            viewport: 1,
            errors_only: false,
            flash: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open the page on `entries` (any order — they are sorted here, newest
    /// first). Opens even when the log is empty: an empty page that says so
    /// answers "did anything happen?" better than a message on the bar the
    /// page is about.
    pub fn open(&mut self, mut entries: Vec<Entry>) {
        entries.sort_by_key(|e| std::cmp::Reverse(e.at));
        self.entries = entries;
        self.cursor = 0;
        self.offset = 0;
        self.errors_only = false;
        self.flash = None;
        self.open = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.flash = None;
    }

    /// The entries the current filter shows, newest first.
    fn listed(&self) -> Vec<&Entry> {
        self.entries
            .iter()
            .filter(|e| !self.errors_only || e.is_error())
            .collect()
    }

    /// The log text of what is listed — what `Y` copies and what `o` hands to
    /// the editor. Chronological (oldest first): a pasted log reads forwards.
    pub fn log_text(&self) -> String {
        format_log(&self.listed())
    }

    /// Scroll, filter, copy or close.
    pub fn handle_key(&mut self, key: &str) -> CenterOutcome {
        // Any key retires the previous copy confirmation, so it is always the
        // answer to the last thing pressed.
        self.flash = None;
        let len = self.listed().len();
        let page = self.viewport.saturating_sub(1).max(1) as isize;
        match key {
            "j" | "down" | "ctrl+j" => self.move_cursor(1, len),
            "k" | "up" | "ctrl+k" => self.move_cursor(-1, len),
            "ctrl+d" | "pagedown" | " " => self.move_cursor(page, len),
            "ctrl+u" | "pageup" => self.move_cursor(-page, len),
            "g" | "home" => self.cursor = 0,
            "G" | "end" => self.cursor = len.saturating_sub(1),
            "y" => self.copy_selected(),
            "Y" => self.copy_all(),
            "e" => self.toggle_errors_only(),
            "o" => return CenterOutcome::OpenEditor,
            "esc" | "q" | "ctrl+c" => return CenterOutcome::Close,
            _ => {}
        }
        CenterOutcome::Consumed
    }

    fn move_cursor(&mut self, delta: isize, len: usize) {
        if len == 0 {
            self.cursor = 0;
            return;
        }
        let next = self.cursor as isize + delta;
        self.cursor = next.clamp(0, len as isize - 1) as usize;
    }

    /// Narrow to errors and back. The cursor keeps its *entry* where it can:
    /// filtering to errors while sitting on one should not jump somewhere
    /// else, or the copy that follows copies the wrong message.
    fn toggle_errors_only(&mut self) {
        let held = self.listed().get(self.cursor).map(|e| (e.at, e.alert));
        self.errors_only = !self.errors_only;
        let landed = {
            let listed = self.listed();
            held.and_then(|id| listed.iter().position(|e| (e.at, e.alert) == id))
        };
        self.cursor = landed.unwrap_or(0);
    }

    fn copy_selected(&mut self) {
        let Some(text) = self.listed().get(self.cursor).map(|e| e.message.clone()) else {
            self.flash = Some(Flash {
                ok: false,
                text: "nothing to copy".to_string(),
            });
            return;
        };
        let chars = text.chars().count();
        self.flash = Some(if crate::clipboard::copy(&text) {
            Flash {
                ok: true,
                text: format!("message copied ({chars} chars)"),
            }
        } else {
            Flash {
                ok: false,
                text: "copy failed — no clipboard and no OSC 52".to_string(),
            }
        });
    }

    fn copy_all(&mut self) {
        let count = self.listed().len();
        if count == 0 {
            self.flash = Some(Flash {
                ok: false,
                text: "nothing to copy".to_string(),
            });
            return;
        }
        let text = self.log_text();
        let what = if self.errors_only {
            "errors"
        } else {
            "messages"
        };
        self.flash = Some(if crate::clipboard::copy(&text) {
            Flash {
                ok: true,
                text: format!("{count} {what} copied as a log"),
            }
        } else {
            Flash {
                ok: false,
                text: "copy failed — no clipboard and no OSC 52".to_string(),
            }
        });
    }

    /// Draw the page.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        if !self.open {
            return;
        }
        let theme = Arc::clone(&self.theme);
        let hints = vec![
            ("j/k", "move"),
            ("y", "copy"),
            ("Y", "copy all"),
            ("e", "errors"),
            ("o", "editor"),
            ("esc", "close"),
        ];
        let heading = self.heading(&theme);

        // The body reflows into whatever width it gets, so the room comes
        // first and the shape second.
        let (avail_w, avail_h) = PanelChrome::new(heading.clone())
            .hints(hints.clone())
            .body(MIN_WIDTH, 0)
            .available(area);
        let width = (avail_w as usize)
            .clamp(0, MAX_WIDTH)
            .max(MIN_WIDTH.min(avail_w as usize));

        let lines = self.build_lines(width, &theme);
        let flash_rows = usize::from(self.flash.is_some());
        // One row of air above the flash line keeps it off the last entry.
        let wanted = lines.len() + if flash_rows > 0 { flash_rows + 1 } else { 0 };
        let rows = wanted.min(avail_h as usize).max(1);

        let body = PanelChrome::new(heading)
            .hints(hints)
            .body(width, rows as u16)
            .render(frame, area, &theme);
        let Some(body) = body else { return };

        let list_h = (body.height as usize).saturating_sub(if flash_rows > 0 { 2 } else { 0 });
        self.viewport = list_h.max(1);
        self.scroll_to_cursor(&lines, self.viewport);

        let text_w = body.width as usize;
        for (i, rendered) in lines.iter().skip(self.offset).take(list_h).enumerate() {
            frame.render_widget(
                Paragraph::new(rendered.line.clone()),
                Rect::new(body.x, body.y + i as u16, body.width, 1),
            );
        }

        self.render_scrollbar(frame, body, lines.len(), list_h, &theme);

        if let Some(flash) = &self.flash {
            let y = body.bottom().saturating_sub(1);
            let style = Style::default()
                .fg(if flash.ok {
                    theme.success()
                } else {
                    theme.error()
                })
                .add_modifier(Modifier::BOLD);
            let mark = if flash.ok { "✓ " } else { "✖ " };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(mark, style),
                    Span::styled(
                        truncate(&flash.text, text_w.saturating_sub(2)),
                        Style::default().fg(theme.form_text()),
                    ),
                ])),
                Rect::new(body.x, y, body.width, 1),
            );
        }
    }

    /// `✦ Notifications · 12 messages, 3 errors` — the page says what it holds
    /// before the list does, and says when a filter is hiding something.
    fn heading(&self, theme: &Theme) -> Line<'static> {
        let errors = self.entries.iter().filter(|e| e.is_error()).count();
        let total = self.entries.len();
        let summary = if total == 0 {
            "empty".to_string()
        } else if self.errors_only {
            format!("{} of {total} — errors only", count(errors, "error"))
        } else if errors > 0 {
            format!("{}, {}", count(total, "message"), count(errors, "error"))
        } else {
            count(total, "message")
        };
        Line::from(vec![
            Span::styled(
                "✦ Notifications",
                Style::default()
                    .fg(theme.form_accent())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  ·  {summary}"),
                Style::default().fg(theme.form_hint()),
            ),
        ])
    }

    /// One rendered line per list row: the entry headers, plus the wrapped
    /// body of the entry under the cursor.
    fn build_lines(&self, width: usize, theme: &Theme) -> Vec<Rendered> {
        let listed = self.listed();
        if listed.is_empty() {
            let text = if self.errors_only {
                "No errors in the log — press e for everything."
            } else {
                "Nothing has been reported yet."
            };
            return vec![Rendered {
                index: 0,
                header: true,
                line: Line::from(Span::styled(
                    text.to_string(),
                    Style::default().fg(theme.form_hint()),
                )),
            }];
        }

        let text_w = width.saturating_sub(TEXT_COL + BAR_COL).max(8);
        let row_w = width.saturating_sub(BAR_COL) as u16;
        // Stamps are `HH:MM:SS`, which says nothing about *which* day — so a
        // log that spans more than one gets a dated rule where the day turns.
        let dated = listed
            .first()
            .zip(listed.last())
            .is_some_and(|(a, b)| a.at.date_naive() != b.at.date_naive());
        let mut day = None;
        let mut out = Vec::new();
        for (i, entry) in listed.iter().enumerate() {
            if dated && day != Some(entry.at.date_naive()) {
                day = Some(entry.at.date_naive());
                out.push(Rendered {
                    index: usize::MAX,
                    header: false,
                    line: day_rule(entry, width.saturating_sub(BAR_COL), theme),
                });
            }
            let selected = i == self.cursor;
            let row_bg = selected.then(|| cursor_bg(theme));
            let base = match row_bg {
                Some(bg) => Style::default().bg(bg),
                None => Style::default(),
            };
            let (glyph, glyph_color) = marker(entry, theme);
            let first = entry.message.lines().next().unwrap_or("");
            let spans = vec![
                Span::styled(
                    if selected { GUTTER } else { "  " },
                    base.fg(theme.form_accent()),
                ),
                Span::styled(
                    entry.at.format("%H:%M:%S").to_string(),
                    base.fg(theme.form_hint()),
                ),
                Span::styled("  ", base),
                Span::styled(glyph.to_string(), base.fg(glyph_color)),
                Span::styled(" ", base),
                Span::styled(
                    truncate(first, text_w),
                    if entry.is_error() {
                        base.fg(theme.form_text()).add_modifier(Modifier::BOLD)
                    } else {
                        base.fg(theme.form_text())
                    },
                ),
            ];
            out.push(Rendered {
                index: i,
                header: true,
                line: pad_row(spans, row_w, base),
            });

            // Only the cursor row unfolds, and only when folding actually hid
            // something — repeating a short one-liner underneath itself would
            // be noise.
            if !selected {
                continue;
            }
            let wrapped = wrap(&entry.message, text_w);
            if wrapped.len() == 1 && wrapped[0] == first {
                continue;
            }
            for line in wrapped {
                out.push(Rendered {
                    index: i,
                    header: false,
                    line: pad_row(
                        vec![
                            Span::styled(GUTTER, base.fg(theme.form_accent())),
                            Span::styled(" ".repeat(TEXT_COL - 4), base),
                            Span::styled("│ ", base.fg(theme.form_hint())),
                            Span::styled(line, base.fg(theme.form_text())),
                        ],
                        row_w,
                        base,
                    ),
                });
            }
        }
        out
    }

    /// Keep the cursor's entry on screen: its header always, and as much of
    /// its unfolded body as the viewport has room for.
    fn scroll_to_cursor(&mut self, lines: &[Rendered], viewport: usize) {
        let Some(head) = lines
            .iter()
            .position(|r| r.index == self.cursor && r.header)
        else {
            self.offset = 0;
            return;
        };
        let tail = lines
            .iter()
            .rposition(|r| r.index == self.cursor)
            .unwrap_or(head);
        let max_offset = lines.len().saturating_sub(viewport);
        if tail >= self.offset + viewport {
            self.offset = tail + 1 - viewport;
        }
        // The header wins over the tail: an entry taller than the viewport
        // scrolls from its top, not from its last line.
        self.offset = self.offset.min(head).min(max_offset);
    }

    /// A one-cell gutter on the right showing where in the log the viewport
    /// sits. Drawn only when there is something to scroll.
    fn render_scrollbar(
        &self,
        frame: &mut Frame,
        body: Rect,
        total: usize,
        viewport: usize,
        theme: &Theme,
    ) {
        if total <= viewport || viewport == 0 || body.width == 0 {
            return;
        }
        let thumb_h = (viewport * viewport).div_ceil(total).max(1);
        let span = viewport.saturating_sub(thumb_h);
        let max_offset = total.saturating_sub(viewport);
        let thumb_y = if max_offset == 0 {
            0
        } else {
            (self.offset * span).div_ceil(max_offset)
        };
        let x = body.right().saturating_sub(1);
        for i in 0..viewport {
            let (glyph, color) = if i >= thumb_y && i < thumb_y + thumb_h {
                ("┃", theme.form_accent())
            } else {
                ("│", theme.form_hint())
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(glyph, Style::default().fg(color)))),
                Rect::new(x, body.y + i as u16, 1, 1),
            );
        }
    }
}

/// One drawn line and the entry it belongs to.
struct Rendered {
    index: usize,
    header: bool,
    line: Line<'static>,
}

/// `1 error` / `3 errors` — a count that reads like a sentence.
fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{n} {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// The dated rule that opens a day in a log spanning several.
fn day_rule(entry: &Entry, width: usize, theme: &Theme) -> Line<'static> {
    let label = format!("── {} ", entry.at.format("%A, %-d %B %Y"));
    let rule = width.saturating_sub(label.chars().count());
    Line::from(Span::styled(
        format!("{label}{}", "─".repeat(rule)),
        Style::default().fg(theme.form_hint()),
    ))
}

/// The marker glyph and colour for an entry: what it is, at a glance.
fn marker(entry: &Entry, theme: &Theme) -> (char, ratatui::style::Color) {
    if entry.is_error() {
        ('✖', theme.error())
    } else if entry.alert {
        ('▲', theme.warning())
    } else {
        ('●', theme.form_accent())
    }
}

/// The plain-text log: one `[stamp] message` block per entry, oldest first,
/// alert lines marked with `!` and continuation lines indented under their
/// stamp. This is what both the clipboard and `$EDITOR` get.
pub fn format_log(entries: &[&Entry]) -> String {
    let mut entries: Vec<&&Entry> = entries.iter().collect();
    entries.sort_by_key(|e| e.at);

    let mut out = String::new();
    for entry in entries {
        let stamp = entry.at.format("%Y-%m-%d %H:%M:%S");
        let mark = if entry.alert { "! " } else { "" };
        if entry.message.is_empty() {
            out.push_str(&format!("[{stamp}] {mark}\n"));
            continue;
        }
        for (i, line) in entry.message.lines().enumerate() {
            if i == 0 {
                out.push_str(&format!("[{stamp}] {mark}{line}\n"));
            } else {
                out.push_str(&format!("    {line}\n"));
            }
        }
    }
    out
}

/// Cut `text` to `width` cells, marking the cut with an ellipsis.
fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }
    let mut out: String = text.chars().take(width - 1).collect();
    out.push('…');
    out
}

/// Wrap on words, keeping explicit line breaks, and breaking mid-word when a
/// word is wider than the line — an error message is full of URLs and paths
/// that no word boundary would ever split.
pub(crate) fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for raw in text.lines() {
        let mut current = String::new();
        for word in raw.split_whitespace() {
            let wlen = word.chars().count();
            if !current.is_empty() && current.chars().count() + 1 + wlen > width {
                out.push(std::mem::take(&mut current));
            }
            if wlen > width {
                let mut rest: &str = word;
                while rest.chars().count() > width {
                    let room = width - current.chars().count();
                    let take: String = rest.chars().take(room).collect();
                    current.push_str(&take);
                    out.push(std::mem::take(&mut current));
                    rest = &rest[take.len()..];
                }
                current.push_str(rest);
                continue;
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
        out.push(current);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use ratatui::{Terminal, backend::TestBackend};

    fn theme() -> Arc<Theme> {
        Arc::new(Theme::new(crate::config::ThemeConfig::default()))
    }

    fn entry(minute: u32, message: &str, level: NoticeLevel, alert: bool) -> Entry {
        Entry {
            at: chrono::Local
                .with_ymd_and_hms(2026, 8, 3, 9, minute, 0)
                .unwrap(),
            message: message.to_string(),
            level,
            alert,
        }
    }

    fn info(minute: u32, message: &str) -> Entry {
        entry(minute, message, NoticeLevel::Info, false)
    }

    fn error(minute: u32, message: &str) -> Entry {
        entry(minute, message, NoticeLevel::Error, false)
    }

    fn screen(center: &mut NotificationCenter, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| center.render(f, f.area())).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf.cell((x, y)).unwrap().symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn open_with(entries: Vec<Entry>) -> NotificationCenter {
        let mut center = NotificationCenter::new(theme());
        center.open(entries);
        center
    }

    #[test]
    fn the_newest_message_is_the_one_the_cursor_starts_on() {
        let mut center = open_with(vec![info(0, "older"), info(5, "newer")]);
        let text = screen(&mut center, 80, 24);
        let newer = text.find("newer").expect("newer shown");
        let older = text.find("older").expect("older shown");
        assert!(newer < older, "newest first:\n{text}");
        assert_eq!(
            center.listed()[center.cursor].message,
            "newer",
            "the cursor opens on the newest message"
        );
    }

    #[test]
    fn the_cursor_row_unfolds_what_the_single_line_cut_off() {
        let long = format!("headline\n{}", "detail ".repeat(30));
        let mut center = open_with(vec![info(0, &long)]);
        let text = screen(&mut center, 80, 24);
        assert!(text.contains("headline"), "the header line:\n{text}");
        assert!(
            text.contains("detail detail"),
            "the folded-away rest is shown for the cursor row:\n{text}"
        );
        assert!(
            text.contains('│'),
            "the unfolded block has a gutter:\n{text}"
        );
    }

    #[test]
    fn a_one_liner_is_not_repeated_under_itself() {
        let mut center = open_with(vec![info(0, "short one")]);
        let text = screen(&mut center, 80, 24);
        assert_eq!(
            text.matches("short one").count(),
            1,
            "a message that fits needs no unfolding:\n{text}"
        );
    }

    #[test]
    fn errors_only_hides_the_rest_and_says_so() {
        let mut center = open_with(vec![info(0, "just saying"), error(5, "it broke")]);
        assert_eq!(center.handle_key("e"), CenterOutcome::Consumed);
        let text = screen(&mut center, 80, 24);
        assert!(text.contains("it broke"), "the error stays:\n{text}");
        assert!(!text.contains("just saying"), "the rest is gone:\n{text}");
        assert!(
            text.contains("errors only"),
            "the heading names the filter:\n{text}"
        );
    }

    #[test]
    fn filtering_keeps_the_cursor_on_the_message_it_was_on() {
        let mut center = open_with(vec![
            error(0, "first error"),
            info(5, "chatter"),
            error(10, "second error"),
        ]);
        center.handle_key("j"); // chatter
        center.handle_key("j"); // first error (newest first: 10, 5, 0)
        assert_eq!(center.listed()[center.cursor].message, "first error");
        center.handle_key("e");
        assert_eq!(
            center.listed()[center.cursor].message,
            "first error",
            "the entry under the cursor survives the filter"
        );
    }

    #[test]
    fn an_empty_filter_result_says_how_to_get_back() {
        let mut center = open_with(vec![info(0, "just saying")]);
        center.handle_key("e");
        let text = screen(&mut center, 80, 24);
        assert!(text.contains("No errors in the log"), "{text}");
    }

    #[test]
    fn an_empty_log_still_opens_with_a_word() {
        let mut center = open_with(Vec::new());
        assert!(center.is_open());
        let text = screen(&mut center, 80, 24);
        assert!(text.contains("Nothing has been reported yet"), "{text}");
    }

    #[test]
    fn the_cursor_stops_at_both_ends() {
        let mut center = open_with(vec![info(0, "a"), info(5, "b")]);
        for _ in 0..5 {
            center.handle_key("k");
        }
        assert_eq!(center.cursor, 0);
        for _ in 0..5 {
            center.handle_key("j");
        }
        assert_eq!(center.cursor, 1);
    }

    #[test]
    fn esc_closes_and_o_hands_the_log_to_the_editor() {
        let mut center = open_with(vec![info(0, "a")]);
        assert_eq!(center.handle_key("o"), CenterOutcome::OpenEditor);
        assert_eq!(center.handle_key("esc"), CenterOutcome::Close);
    }

    #[test]
    fn the_log_merges_both_bars_chronologically() {
        let center = open_with(vec![
            info(5, "second"),
            info(20, "fourth"),
            entry(0, "first", NoticeLevel::Info, true),
            entry(10, "third", NoticeLevel::Error, true),
        ]);
        assert_eq!(
            center.log_text(),
            "[2026-08-03 09:00:00] ! first\n\
             [2026-08-03 09:05:00] second\n\
             [2026-08-03 09:10:00] ! third\n\
             [2026-08-03 09:20:00] fourth\n"
        );
    }

    #[test]
    fn the_log_indents_continuation_lines() {
        let center = open_with(vec![info(0, "headline\ndetail")]);
        assert_eq!(
            center.log_text(),
            "[2026-08-03 09:00:00] headline\n    detail\n"
        );
    }

    #[test]
    fn the_log_is_empty_without_entries() {
        let center = open_with(Vec::new());
        assert!(center.log_text().is_empty());
    }

    /// What the page shows is what `Y` and `o` hand out — copying a filtered
    /// page must not smuggle the hidden messages along.
    #[test]
    fn the_log_follows_the_filter() {
        let mut center = open_with(vec![info(0, "chatter"), error(5, "it broke")]);
        center.handle_key("e");
        let text = center.log_text();
        assert!(text.contains("it broke"));
        assert!(!text.contains("chatter"));
    }

    #[test]
    fn a_word_wider_than_the_line_is_broken_rather_than_lost() {
        let url = "https://example.invalid/".to_string() + &"a".repeat(60);
        let lines = wrap(&url, 20);
        assert!(lines.len() > 1, "the URL wraps: {lines:?}");
        assert!(lines.iter().all(|l| l.chars().count() <= 20));
        assert_eq!(lines.concat(), url, "no character is dropped");
    }

    #[test]
    fn truncation_is_marked() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }
}
