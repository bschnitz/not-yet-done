//! NotificationBar component: persistent message area below the table.
//!
//! Notifications accumulate until dismissed. They are word-wrapped and
//! limited to a configurable maximum number of lines and messages.
//!
//! Everything pushed is also kept in a timestamped [`history`](NotificationBarComponent::history)
//! that survives both the display cap and a dismiss, so a message that scrolled
//! out of the (possibly one-line) bar can still be read afterwards — see
//! `GlobalAction::ShowNotifications`.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};

use tuirealm::command::{Cmd, CmdResult};
use tuirealm::component::Component;
use tuirealm::props::{AttrValue, Attribute, QueryResult};
use tuirealm::state::{State, StateValue};

use crate::ui::theme::Theme;
use std::sync::Arc;

/// One entry of the bar's notification log: the message plus the local time
/// it was pushed. Kept even after the message left the bar.
#[derive(Debug, Clone)]
pub struct NotificationRecord {
    pub at: chrono::DateTime<chrono::Local>,
    pub message: String,
}

/// What a message *is* — the order in which the bar resolves scarcity.
///
/// The bar caps its height, and what does not fit is not drawn. Sorting by
/// class before capping makes that eviction predictable: the least actionable
/// message goes first, never the most. Declaration order is the priority.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum NoticeClass {
    /// Something a person said or must answer — a notification, an error, an
    /// MFA challenge. The bar's ordinary content.
    #[default]
    Message,
    /// A load counter: synthesised, self-retracting, and readable again from
    /// the row it describes. The first thing to drop when the bar is full,
    /// and deliberately kept out of the notification log — it ticks once a
    /// second and would bury everything worth keeping.
    Load,
}

/// One entry on the bar.
#[derive(Debug, Clone)]
struct Notice {
    /// Identity of the *sender*, when it has one. A keyed notice is
    /// overwritten in place by the next `set_keyed` and retracted by
    /// `clear_keyed`, so a live message can update without the sender having
    /// to remember its own last exact string — and two senders whose texts
    /// coincide can no longer retract each other.
    key: Option<String>,
    class: NoticeClass,
    text: String,
}

pub struct NotificationBarComponent {
    theme: Arc<Theme>,
    messages: Vec<Notice>,
    max_lines: u16,
    /// How many messages the bar shows at once. Pushing past this drops the
    /// *oldest* so the newest message is always the visible one — without it a
    /// short bar would freeze on the first message and hide everything after.
    /// `None` = unbounded (the top alert bar's behaviour).
    max_messages: Option<usize>,
    /// Timestamped log of every message pushed, newest last. Independent of
    /// what is currently displayed: unaffected by the `max_messages` cap and
    /// kept across [`Self::clear`], so `show_notifications` can open the whole
    /// backlog in an editor. Trimmed from the front at `history_limit`.
    history: Vec<NotificationRecord>,
    history_limit: Option<usize>,
    /// Right-aligned hint on the bar's last row. Set from the live keybindings
    /// so it names the keys that actually dismiss the bar and open the log,
    /// instead of hard-coding them.
    hint: String,
    /// When set, the bar renders in its loud "alert" style — bold text on the
    /// theme's high-contrast `alert_fg`/`alert_bg` colours with a `▲` marker —
    /// instead of the muted default (surface background, `●` bullet). Used by
    /// the prominent top bar. `None` = ordinary bottom notification bar.
    prominent: bool,
}

impl NotificationBarComponent {
    pub fn new(theme: Arc<Theme>) -> Self {
        Self {
            theme,
            messages: Vec::new(),
            max_lines: 5,
            max_messages: None,
            history: Vec::new(),
            history_limit: Some(200),
            hint: "[Z] dismiss".to_string(),
            prominent: false,
        }
    }

    /// Replace the right-aligned hint (see [`Self::hint`]). Empty hides it.
    pub fn set_hint(&mut self, hint: String) {
        self.hint = hint;
    }

    pub fn set_max_lines(&mut self, max: u16) {
        self.max_lines = max;
    }

    /// Cap the number of *displayed* messages. `0` means unlimited.
    pub fn set_max_messages(&mut self, max: u16) {
        self.max_messages = (max > 0).then_some(max as usize);
        self.trim_messages();
    }

    /// Cap the retained notification log. `0` means unlimited.
    pub fn set_history_limit(&mut self, max: u16) {
        self.history_limit = (max > 0).then_some(max as usize);
        self.trim_history();
    }

    /// Switch this bar into the loud "alert" presentation (see [`Self::prominent`]).
    pub fn set_prominent(&mut self, prominent: bool) {
        self.prominent = prominent;
    }

    pub fn push(&mut self, msg: String) {
        self.log(&msg);
        self.messages.push(Notice {
            key: None,
            class: NoticeClass::Message,
            text: msg,
        });
        self.trim_messages();
    }

    /// Show `text` under `key`, replacing whatever that key showed before.
    ///
    /// This is how a live message updates: the sender names itself once and
    /// overwrites its own slot, instead of retracting its last exact string
    /// and appending a new one. Two senders can therefore show identical text
    /// without either dismissing the other's, and a sender that forgets to
    /// clear leaks one slot rather than one message per update.
    ///
    /// A slot keeps its position while it lives, so a ticking counter does not
    /// jump around; the log records it on arrival and on every change of text,
    /// except for [`NoticeClass::Load`], which is never logged.
    pub fn set_keyed(&mut self, key: &str, class: NoticeClass, text: String) {
        if class != NoticeClass::Load
            && self
                .messages
                .iter()
                .all(|n| n.key.as_deref() != Some(key) || n.text != text)
        {
            self.log(&text);
        }
        if let Some(slot) = self
            .messages
            .iter_mut()
            .find(|n| n.key.as_deref() == Some(key))
        {
            slot.class = class;
            slot.text = text;
            return;
        }
        self.messages.push(Notice {
            key: Some(key.to_string()),
            class,
            text,
        });
        self.trim_messages();
    }

    /// Retract the slot `key` owns. Unknown keys are a no-op — a sender may
    /// clear a message it never got to show.
    pub fn clear_keyed(&mut self, key: &str) {
        self.messages.retain(|n| n.key.as_deref() != Some(key));
    }

    fn log(&mut self, msg: &str) {
        self.history.push(NotificationRecord {
            at: chrono::Local::now(),
            message: msg.to_string(),
        });
        self.trim_history();
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// The retained notification log, newest last.
    pub fn history(&self) -> &[NotificationRecord] {
        &self.history
    }

    /// Move the displayed messages and the notification log over from a bar
    /// that is being replaced. Reloading `tui.yaml` rebuilds the components
    /// (they hold theme refs), and without this every pending message and the
    /// whole log would vanish on `:config` save.
    pub fn adopt_state(&mut self, other: &Self) {
        self.messages = other.messages.clone();
        self.history = other.history.clone();
        self.trim_messages();
        self.trim_history();
    }

    fn trim_messages(&mut self) {
        if let Some(max) = self.max_messages {
            let len = self.messages.len();
            if len > max {
                self.messages.drain(..len - max);
            }
        }
    }

    fn trim_history(&mut self) {
        if let Some(max) = self.history_limit {
            let len = self.history.len();
            if len > max {
                self.history.drain(..len - max);
            }
        }
    }

    #[allow(dead_code)]
    pub fn is_visible(&self) -> bool {
        !self.messages.is_empty()
    }

    #[allow(dead_code)]
    pub fn set_message(&mut self, msg: Option<String>) {
        self.messages.clear();
        if let Some(m) = msg {
            self.messages.push(Notice {
                key: None,
                class: NoticeClass::Message,
                text: m,
            });
        }
    }

    /// Calculate the number of rows needed to display all messages.
    pub fn required_height(&self, available_width: u16) -> u16 {
        if self.messages.is_empty() {
            return 0;
        }
        let w = available_width.saturating_sub(2) as usize; // 1 char padding each side
        if w == 0 {
            return 0;
        }

        let mut total_lines: u16 = 0;
        for n in &self.messages {
            total_lines = total_lines.saturating_add(wrap_lines(&n.text, w).len() as u16);
        }
        total_lines.min(self.max_lines).max(1)
    }

    /// The messages in the order they are drawn: by class, insertion order
    /// within a class. Stable, so a slot only moves when its class does.
    fn ordered(&self) -> Vec<&Notice> {
        let mut out: Vec<&Notice> = self.messages.iter().collect();
        out.sort_by_key(|n| n.class);
        out
    }

    /// Lay the messages out into at most `rows` lines, and count how many of
    /// them the cap leaves incompletely shown.
    ///
    /// A message counts as hidden as soon as *any* of its lines is dropped:
    /// half a sentence is not a message the user has read, and the `(+N more)`
    /// marker exists precisely so truncation stops being silent.
    fn layout(&self, width: usize, rows: usize) -> (Vec<BarLine>, usize) {
        let mut out: Vec<BarLine> = Vec::new();
        let mut hidden = 0usize;
        for n in self.ordered() {
            let lines = wrap_lines(&n.text, width);
            let room = rows.saturating_sub(out.len());
            if room < lines.len() {
                hidden += 1;
            }
            for (i, line) in lines.into_iter().take(room).enumerate() {
                out.push(BarLine {
                    first: i == 0,
                    text: line,
                });
            }
        }
        (out, hidden)
    }
}

/// One drawn row: its text and whether it opens a message (bullet) or
/// continues one (indent).
struct BarLine {
    first: bool,
    text: String,
}

impl Component for NotificationBarComponent {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        if self.messages.is_empty() || area.height == 0 {
            return;
        }
        let t = &self.theme;
        let (bg, fg, bullet, bullet_fg): (Color, Color, char, Color) = if self.prominent {
            (t.alert_bg(), t.alert_fg(), '▲', t.alert_fg())
        } else {
            (t.surface(), t.text_high(), '●', t.accent())
        };
        let mut style = Style::default().fg(fg).bg(bg);
        if self.prominent {
            style = style.add_modifier(Modifier::BOLD);
        }
        // Hint stays legible in either style: dim on the muted bar, the alert
        // fg on the loud one (text_dim would vanish against a bright field).
        let dim_style = Style::default()
            .fg(if self.prominent { fg } else { t.text_dim() })
            .bg(bg);
        let buf = frame.buffer_mut();

        // Fill background.
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                    cell.set_char(' ');
                    cell.set_style(Style::default().bg(bg));
                }
            }
        }

        let w = area.width.saturating_sub(2) as usize;
        let (mut lines, hidden) = self.layout(w, area.height as usize);

        // What the cap left out is announced on the last drawn line, so a
        // truncated bar reads as truncated instead of as complete.
        if hidden > 0
            && let Some(last) = lines.last_mut()
        {
            last.text.push_str(&format!(" (+{hidden} more)"));
        }

        let mut y = area.top();
        for line in &lines {
            if y >= area.bottom() {
                break;
            }
            let mut x = area.left() + 1;
            // Show bullet on first line of each message.
            if line.first {
                let bullet_style = Style::default().fg(bullet_fg).bg(bg);
                if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                    cell.set_char(bullet);
                    cell.set_style(bullet_style);
                }
                x += 1;
                if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                    cell.set_char(' ');
                    cell.set_style(style);
                }
                x += 1;
            } else {
                x += 2; // indent continuation lines
            }
            for ch in line.text.chars() {
                if x >= area.right().saturating_sub(1) {
                    break;
                }
                if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                    cell.set_char(ch);
                    cell.set_style(style);
                }
                x += 1;
            }
            y += 1;
        }

        // Dismiss / open-log hint on the last row, right-aligned.
        let hint = self.hint.as_str();
        if hint.is_empty() {
            return;
        }
        let hint_x = area.right().saturating_sub(hint.chars().count() as u16 + 1);
        let hint_y = area.bottom().saturating_sub(1);
        let mut hx = hint_x;
        for ch in hint.chars() {
            if hx >= area.right() {
                break;
            }
            if let Some(cell) = buf.cell_mut(Position::new(hx, hint_y)) {
                cell.set_char(ch);
                cell.set_style(dim_style);
            }
            hx += 1;
        }
    }

    fn query(&self, _attr: Attribute) -> Option<QueryResult<'_>> {
        None
    }
    fn attr(&mut self, _attr: Attribute, _value: AttrValue) {}
    fn state(&self) -> State {
        if self.messages.is_empty() {
            State::None
        } else {
            let texts: Vec<&str> = self.ordered().iter().map(|n| n.text.as_str()).collect();
            State::Single(StateValue::String(texts.join("\n")))
        }
    }
    fn perform(&mut self, _cmd: Cmd) -> CmdResult {
        CmdResult::NoChange
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;
    use ratatui::{Terminal, backend::TestBackend};

    fn theme() -> Arc<Theme> {
        Arc::new(Theme::new(crate::config::ThemeConfig::default()))
    }

    fn render(bar: &mut NotificationBarComponent, w: u16, h: u16) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| bar.view(f, f.area())).unwrap();
        terminal.backend().buffer().clone()
    }

    #[test]
    fn default_bar_uses_bullet_and_surface() {
        let mut bar = NotificationBarComponent::new(theme());
        bar.push("hello".into());
        let buf = render(&mut bar, 40, 1);
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(
            text.contains('●'),
            "default bar should show ● bullet: {text}"
        );
        assert!(
            !text.contains('▲'),
            "default bar must not use the alert marker"
        );
        // First cell background is the muted surface, not the loud alert bg.
        let t = theme();
        assert_eq!(buf.content()[0].bg, t.surface());
    }

    #[test]
    fn max_messages_drops_the_oldest_so_the_newest_shows() {
        let mut bar = NotificationBarComponent::new(theme());
        bar.set_max_messages(1);
        bar.push("older".into());
        bar.push("newer".into());
        let buf = render(&mut bar, 40, 1);
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(
            text.contains("newer"),
            "newest message must be visible: {text}"
        );
        assert!(
            !text.contains("older"),
            "older message must be pushed out: {text}"
        );
        // …but nothing is lost: the log still holds both.
        let logged: Vec<&str> = bar.history().iter().map(|r| r.message.as_str()).collect();
        assert_eq!(logged, vec!["older", "newer"]);
    }

    #[test]
    fn zero_max_messages_means_unlimited() {
        let mut bar = NotificationBarComponent::new(theme());
        bar.set_max_messages(0);
        bar.push("a".into());
        bar.push("b".into());
        assert_eq!(bar.required_height(40), 2);
    }

    #[test]
    fn history_survives_a_dismiss() {
        let mut bar = NotificationBarComponent::new(theme());
        bar.push("gone from the bar".into());
        bar.clear();
        assert!(!bar.is_visible());
        assert_eq!(bar.history().len(), 1);
    }

    #[test]
    fn history_limit_trims_the_oldest_entries() {
        let mut bar = NotificationBarComponent::new(theme());
        bar.set_history_limit(2);
        for msg in ["a", "b", "c"] {
            bar.push(msg.into());
        }
        let logged: Vec<&str> = bar.history().iter().map(|r| r.message.as_str()).collect();
        assert_eq!(logged, vec!["b", "c"]);
    }

    #[test]
    fn adopt_state_carries_messages_and_log_to_the_rebuilt_bar() {
        let mut old = NotificationBarComponent::new(theme());
        old.push("pending".into());
        let mut fresh = NotificationBarComponent::new(theme());
        fresh.adopt_state(&old);
        assert!(fresh.is_visible());
        assert_eq!(fresh.history().len(), 1);
    }

    #[test]
    fn a_keyed_slot_is_overwritten_in_place_not_appended() {
        let mut bar = NotificationBarComponent::new(theme());
        bar.set_keyed("load:0", NoticeClass::Load, "Loading… (1s)".into());
        bar.set_keyed("load:0", NoticeClass::Load, "Loading… (2s)".into());
        assert_eq!(bar.required_height(40), 1);
        let text: String = render(&mut bar, 40, 1)
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            text.contains("(2s)"),
            "the slot shows its latest text: {text}"
        );
    }

    #[test]
    fn clearing_one_slot_leaves_an_identically_worded_one_alone() {
        let mut bar = NotificationBarComponent::new(theme());
        bar.set_keyed("a", NoticeClass::Message, "Waiting for input".into());
        bar.set_keyed("b", NoticeClass::Message, "Waiting for input".into());
        bar.clear_keyed("a");
        assert_eq!(bar.required_height(40), 1, "exactly one of the two is left");
    }

    #[test]
    fn load_counters_stay_out_of_the_notification_log() {
        let mut bar = NotificationBarComponent::new(theme());
        bar.set_keyed("load:0", NoticeClass::Load, "Loading… (1s)".into());
        bar.set_keyed("load:0", NoticeClass::Load, "Loading… (2s)".into());
        bar.set_keyed("mfa", NoticeClass::Message, "Approve the sign-in".into());
        let logged: Vec<&str> = bar.history().iter().map(|r| r.message.as_str()).collect();
        assert_eq!(logged, vec!["Approve the sign-in"]);
    }

    #[test]
    fn a_full_bar_evicts_the_load_counter_before_the_message() {
        let mut bar = NotificationBarComponent::new(theme());
        bar.set_max_lines(1);
        bar.set_keyed("load:0", NoticeClass::Load, "Loading".into());
        bar.push("Approve the sign-in".into());
        let text: String = render(&mut bar, 40, 1)
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            text.contains("Approve"),
            "the actionable message survives: {text}"
        );
        assert!(
            !text.contains("Loading"),
            "the counter is the one dropped: {text}"
        );
    }

    #[test]
    fn what_the_cap_leaves_out_is_announced() {
        let mut bar = NotificationBarComponent::new(theme());
        bar.set_max_lines(1);
        bar.push("first".into());
        bar.push("second".into());
        bar.push("third".into());
        let text: String = render(&mut bar, 40, 1)
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            text.contains("(+2 more)"),
            "truncation must be visible: {text}"
        );
    }

    #[test]
    fn hint_is_configurable_and_hideable() {
        let mut bar = NotificationBarComponent::new(theme());
        bar.set_hint("[Q] quit".into());
        bar.push("hello".into());
        let text: String = render(&mut bar, 40, 1)
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            text.contains("[Q] quit"),
            "custom hint should render: {text}"
        );

        bar.set_hint(String::new());
        let text: String = render(&mut bar, 40, 1)
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            !text.contains('['),
            "empty hint should render nothing: {text}"
        );
    }

    #[test]
    fn prominent_bar_uses_alert_marker_colors_and_bold() {
        let mut bar = NotificationBarComponent::new(theme());
        bar.set_prominent(true);
        bar.push("tap 42".into());
        let buf = render(&mut bar, 40, 1);
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(
            text.contains('▲'),
            "prominent bar should show ▲ marker: {text}"
        );
        let t = theme();
        // The strip is painted with the loud alert background…
        assert_eq!(buf.content()[0].bg, t.alert_bg());
        // …and the message text is bold. Find a text cell and check the modifier.
        let msg_cell = buf
            .content()
            .iter()
            .find(|c| c.symbol() == "t")
            .expect("message text present");
        assert!(
            msg_cell.modifier.contains(Modifier::BOLD),
            "prominent message text must be bold"
        );
        assert_eq!(msg_cell.fg, t.alert_fg());
    }
}

fn wrap_lines(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    for raw_line in text.lines() {
        if raw_line.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in raw_line.split_whitespace() {
            if current.is_empty() {
                current = word.to_string();
            } else if current.len() + 1 + word.len() > max_width {
                lines.push(current);
                current = word.to_string();
            } else {
                current.push(' ');
                current.push_str(word);
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}
