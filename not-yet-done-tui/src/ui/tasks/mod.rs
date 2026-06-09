pub mod forest;
pub mod highlight;
pub mod sort;
pub mod view_helpers;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    Frame,
};

use tuirealm::component::Component;

use crate::app::App;
use crate::config::SplitType;

/// Render the tasks content area (view + optional form).
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let error_height = app.query_error_bar.required_height(area.width);

    let mut constraints = Vec::new();
    if error_height > 0 {
        constraints.push(Constraint::Length(error_height));
    }
    constraints.push(Constraint::Fill(1));

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let mut idx = 0;

    if error_height > 0 {
        app.query_error_bar.view(frame, rows[idx]);
        idx += 1;
    }

    render_content(frame, rows[idx], app);
}

fn render_view_pane(frame: &mut Frame, area: Rect, app: &mut App) {
    // Can't call app.view_pane.render_with_app(app) due to borrow rules.
    // Inline the view_pane logic here.
    crate::components::view_pane::render_view(frame, area, app);
}

fn render_form_pane(frame: &mut Frame, area: Rect, app: &App) {
    app.form_pane.render_with_app(frame, area, app);
}

fn render_content(frame: &mut Frame, area: Rect, app: &mut App) {
    let split_cfg = &app.config.layout.tasks.split;
    let form_open = app.tasks_view.state.form_visible();
    let term = frame.area();

    let split_active = form_open
        && match split_cfg.split_type {
            SplitType::Vertical => term.width >= split_cfg.vertical_threshold,
            SplitType::Horizontal => term.height >= split_cfg.horizontal_threshold,
        };

    if !split_active {
        if form_open {
            render_form_pane(frame, area, app);
        } else {
            render_view_pane(frame, area, app);
        }
        return;
    }

    let split_type = split_cfg.split_type.clone();
    let order = split_cfg.order.clone();
    match split_type {
        SplitType::Vertical => render_split(frame, area, app, Direction::Horizontal, 60, 40, &order),
        SplitType::Horizontal => render_split(frame, area, app, Direction::Vertical, 65, 35, &order),
    }
}

fn render_split(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    direction: Direction,
    view_pct: u16,
    form_pct: u16,
    order: &[crate::config::SplitPane],
) {
    use crate::config::SplitPane;
    let constraints = match order.first() {
        Some(SplitPane::Form) => [
            Constraint::Percentage(form_pct),
            Constraint::Percentage(view_pct),
        ],
        _ => [
            Constraint::Percentage(view_pct),
            Constraint::Percentage(form_pct),
        ],
    };
    let panes = Layout::default()
        .direction(direction)
        .constraints(constraints)
        .split(area);

    let (view_area, form_area) = match order.first() {
        Some(SplitPane::Form) => (panes[1], panes[0]),
        _ => (panes[0], panes[1]),
    };
    render_view_pane(frame, view_area, app);
    render_form_pane(frame, form_area, app);
}
