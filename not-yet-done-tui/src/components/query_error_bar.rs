//! QueryErrorBar component: persistent error bar shown below the sub-tab bar.
//!
//! Unlike NotificationBar (auto-dismissing), this stays visible until the
//! query error is resolved.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Paragraph, Widget, Wrap};

use tuirealm::command::{Cmd, CmdResult};
use tuirealm::component::Component;
use tuirealm::props::{AttrValue, Attribute, QueryResult};
use tuirealm::state::{State, StateValue};

use crate::ui::theme::Theme;
use std::sync::Arc;

pub struct QueryErrorBarComponent {
    theme: Arc<Theme>,
    error: Option<String>,
}

impl QueryErrorBarComponent {
    pub fn new(theme: Arc<Theme>) -> Self {
        Self { theme, error: None }
    }

    pub fn set_error(&mut self, err: Option<String>) {
        self.error = err;
    }

    #[allow(dead_code)]
    pub fn is_visible(&self) -> bool {
        self.error.is_some()
    }

    /// Compute the required height for this error bar given a width.
    pub fn required_height(&self, width: u16) -> u16 {
        let Some(err) = &self.error else { return 0 };
        let w = width.saturating_sub(4) as usize;
        if w == 0 {
            return 1;
        }
        err.lines()
            .map(|line| ((line.chars().count() / w.max(1)) + 1) as u16)
            .sum::<u16>()
            .max(1)
    }
}

impl Component for QueryErrorBarComponent {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let Some(err) = &self.error else { return };
        let t = &self.theme;
        let style = Style::default().fg(t.bg()).bg(t.error());
        let text = format!(" ⚠ {err}");
        Paragraph::new(text)
            .style(style)
            .wrap(Wrap { trim: false })
            .render(area, frame.buffer_mut());
    }

    fn query(&self, _attr: Attribute) -> Option<QueryResult<'_>> {
        None
    }
    fn attr(&mut self, _attr: Attribute, _value: AttrValue) {}
    fn state(&self) -> State {
        match &self.error {
            Some(e) => State::Single(StateValue::String(e.clone())),
            None => State::None,
        }
    }
    fn perform(&mut self, _cmd: Cmd) -> CmdResult {
        CmdResult::NoChange
    }
}
