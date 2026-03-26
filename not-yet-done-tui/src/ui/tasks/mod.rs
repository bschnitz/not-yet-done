// Pfad: not-yet-done-tui/src/ui/tasks/mod.rs

pub mod forest;
pub mod form_pane;
pub mod sub_tab_bar;
pub mod view_pane;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    Frame,
};

use crate::app::App;
use crate::config::SplitType;

use form_pane::TasksFormPane;
use sub_tab_bar::TasksSubTabBar;
use view_pane::TasksViewPane;

/// Entry point called from render.rs for the Tasks tab area.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    // ── Layout: sub-tab-bar (1 row) + content area ───────────────────────
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Fill(1)])
        .split(area);

    frame.render_widget(TasksSubTabBar::new(app), rows[0]);
    render_content(frame, rows[1], app);
}

fn render_content(frame: &mut Frame, area: Rect, app: &App) {
    let split_cfg = &app.config.layout.tasks.split;
    let form_open = app.tasks_state.form_visible();

    let term = frame.area();

    let split_active = form_open && match split_cfg.split_type {
        SplitType::Vertical   => term.width  >= split_cfg.vertical_threshold,
        SplitType::Horizontal => term.height >= split_cfg.horizontal_threshold,
    };

    if !split_active {
        if form_open {
            frame.render_widget(TasksFormPane::new(app), area);
        } else {
            frame.render_widget(TasksViewPane::new(app), area);
        }
        return;
    }

    match split_cfg.split_type {
        SplitType::Vertical   => render_vertical_split(frame, area, app),
        SplitType::Horizontal => render_horizontal_split(frame, area, app),
    }
}

fn render_vertical_split(frame: &mut Frame, area: Rect, app: &App) {
    let split_cfg = &app.config.layout.tasks.split;
    let (view_pct, form_pct) = (60, 40);

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(ordered_constraints(split_cfg, view_pct, form_pct))
        .split(area);

    let (view_area, form_area) = ordered_areas(&split_cfg.order, panes[0], panes[1]);
    frame.render_widget(TasksViewPane::new(app), view_area);
    frame.render_widget(TasksFormPane::new(app), form_area);
}

fn render_horizontal_split(frame: &mut Frame, area: Rect, app: &App) {
    let split_cfg = &app.config.layout.tasks.split;
    let (view_pct, form_pct) = (65, 35);

    let panes = Layout::default()
        .direction(Direction::Vertical)
        .constraints(ordered_constraints(split_cfg, view_pct, form_pct))
        .split(area);

    let (view_area, form_area) = ordered_areas(&split_cfg.order, panes[0], panes[1]);
    frame.render_widget(TasksViewPane::new(app), view_area);
    frame.render_widget(TasksFormPane::new(app), form_area);
}

fn ordered_constraints(
    split_cfg: &crate::config::layout::SplitConfig,
    view_pct: u16,
    form_pct: u16,
) -> [Constraint; 2] {
    use crate::config::SplitPane;
    match split_cfg.order.first() {
        Some(SplitPane::Form) => [
            Constraint::Percentage(form_pct),
            Constraint::Percentage(view_pct),
        ],
        _ => [
            Constraint::Percentage(view_pct),
            Constraint::Percentage(form_pct),
        ],
    }
}

fn ordered_areas(
    order: &[crate::config::SplitPane],
    first: Rect,
    second: Rect,
) -> (Rect, Rect) {
    use crate::config::SplitPane;
    match order.first() {
        Some(SplitPane::Form) => (second, first),
        _                     => (first, second),
    }
}
