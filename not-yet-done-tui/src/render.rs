use ratatui::{
    layout::{Constraint, Direction, Layout},
    Frame,
};

use crate::app::App;
use crate::tabs::Tab;
use crate::ui::{
    placeholder::PlaceholderTab,
    status_bar::StatusBar,
    tab_bar::{TabBar, TabSeparator},
    tasks,
    welcome::WelcomeTab,
};

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),  // main tab bar
            Constraint::Length(1),  // separator
            Constraint::Fill(1),    // tab content
            Constraint::Length(1),  // status bar
        ])
        .split(area);

    frame.render_widget(TabBar::new(app),       chunks[0]);
    frame.render_widget(TabSeparator::new(app), chunks[1]);

    match app.active_tab {
        Tab::Welcome   => frame.render_widget(WelcomeTab::new(app), chunks[2]),
        Tab::Tasks     => tasks::render(frame, chunks[2], app),
        Tab::Trackings => frame.render_widget(PlaceholderTab::new(app, "Trackings", "󱦗"), chunks[2]),
    }

    frame.render_widget(StatusBar::new(app), chunks[3]);
}
