//! Example: "New Team Member"
//!
//! A recruitment form demonstrating the Grid widget:
//!
//! ```
//! ╭────────────────────── New Team Member ──────────────────────╮
//! │ ▍ Full Name                  │ ▍ Role Title                │
//! │ ▍                            │ ▍                           │
//! ├──────────────────────────────┼─────────────────────────────┤
//! │ ▍ Bio / About                                              │
//! │ ▍                                                          │
//! ├────────────────────────────────────────────────────────────┤
//! │ ▍ Team                       │ ▍ Expertise                 │
//! │ ▍ none selected              │ ▍ none selected             │
//! ╰────────────────────────────────────────────────────────────╯
//! ```
//!
//! Layout:
//!   - 3 rows × 2 columns
//!   - Row 1 (Bio) is grouped across both columns
//!   - Rounded outer border + simple inner row/column separators
//!
//! Navigation: Tab / Shift+Tab
//! In dropdowns: ↑/↓ navigate, Space toggles selection
//! Ctrl+S: "Submit" – shows a result overlay
//! Esc: quit

use std::time::Duration;

use crossterm::{cursor::SetCursorStyle, execute};
use not_yet_done_ratatui::widgets::{
    grid::{
        BORDER_ROUNDED, BORDER_SIMPLE, BorderPos, CellGroup, Grid, GridChild, GridKeymap,
        TextAnchor,
    },
    multi_choice::{MultiChoice, MultiChoiceKeymap, MultiChoiceStyle, MultiChoiceStyleType},
    text_input::{TextInput, TextInputStyle, TextInputStyleType},
};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
};
use tuirealm::{
    application::{Application, PollStrategy},
    command::{Cmd, CmdResult},
    component::{AppComponent, Component},
    event::{Event, Key, KeyModifiers, NoUserEvent},
    listener::EventListenerCfg,
    props::{AttrValue, Attribute, QueryResult},
    state::{State, StateValue},
};

// ---------------------------------------------------------------------------
// Colour palette
// ---------------------------------------------------------------------------

const BG: Color = Color::Rgb(15, 17, 26);
const PANEL_BG: Color = Color::Rgb(22, 25, 37);
const BORDER_FG: Color = Color::Rgb(60, 70, 110);
const ACCENT: Color = Color::Rgb(120, 160, 255);
const ACCENT_DIM: Color = Color::Rgb(60, 90, 160);
const INPUT_FG: Color = Color::Rgb(210, 218, 240);
const INPUT_BG: Color = Color::Rgb(30, 34, 52);
const PLACEHOLDER: Color = Color::Rgb(70, 78, 110);
const ACTIVE_FG: Color = Color::Rgb(255, 220, 100);
const SELECTED_BG: Color = Color::Rgb(40, 50, 80);

const HINT_FG: Color = Color::Rgb(55, 65, 100);
const SUBMIT_BG: Color = Color::Rgb(100, 200, 140);
const OVERLAY_BG: Color = Color::Rgb(28, 32, 50);
const ERROR_FG: Color = Color::Rgb(240, 90, 90);

// ---------------------------------------------------------------------------
// Static data
// ---------------------------------------------------------------------------

const TEAMS: [&str; 6] = [
    "Frontend", "Backend", "Design", "Data", "DevOps", "Research",
];
const EXPERTISE: [&str; 7] = [
    "Rust",
    "TypeScript",
    "Python",
    "Go",
    "SQL",
    "Figma",
    "Kubernetes",
];

// ---------------------------------------------------------------------------
// Styles
// ---------------------------------------------------------------------------

fn text_inactive() -> TextInputStyle {
    TextInputStyle::new()
        .prefix_color(ACCENT_DIM)
        .set_style(TextInputStyleType::Title, Style::default().fg(ACCENT_DIM))
        .set_style(TextInputStyleType::Input, Style::default().fg(INPUT_FG))
        .placeholder_color(PLACEHOLDER)
        .set_style(TextInputStyleType::Error, Style::default().fg(ERROR_FG))
}

fn text_active() -> TextInputStyle {
    TextInputStyle::new()
        .prefix_color(ACCENT)
        .set_style(
            TextInputStyleType::Title,
            Style::default().fg(ACCENT).bg(INPUT_BG),
        )
        .set_style(
            TextInputStyleType::Input,
            Style::default().fg(INPUT_FG).bg(INPUT_BG),
        )
        .placeholder_color(PLACEHOLDER)
        .set_style(TextInputStyleType::Error, Style::default().fg(ERROR_FG))
}

fn mc_inactive() -> MultiChoiceStyle {
    MultiChoiceStyle::new()
        .prefix_color(ACCENT_DIM)
        .set_style(MultiChoiceStyleType::Title, Style::default().fg(ACCENT_DIM))
        .set_style(MultiChoiceStyleType::Normal, Style::default().fg(INPUT_FG))
        .set_style(
            MultiChoiceStyleType::Selected,
            Style::default().fg(INPUT_FG).bg(SELECTED_BG),
        )
        .set_style(
            MultiChoiceStyleType::SelectedActive,
            Style::default().fg(ACTIVE_FG).bg(SELECTED_BG),
        )
        .set_style(MultiChoiceStyleType::LastLine, Style::default())
}

fn mc_active() -> MultiChoiceStyle {
    MultiChoiceStyle::new()
        .prefix_color(ACCENT)
        .set_style(
            MultiChoiceStyleType::Title,
            Style::default().fg(ACCENT).bg(INPUT_BG),
        )
        .set_style(
            MultiChoiceStyleType::Normal,
            Style::default().fg(INPUT_FG).bg(INPUT_BG),
        )
        .set_style(
            MultiChoiceStyleType::Active,
            Style::default().fg(ACTIVE_FG).bg(INPUT_BG),
        )
        .set_style(
            MultiChoiceStyleType::Selected,
            Style::default().fg(INPUT_FG).bg(SELECTED_BG),
        )
        .set_style(
            MultiChoiceStyleType::SelectedActive,
            Style::default().fg(ACTIVE_FG).bg(SELECTED_BG),
        )
        .set_style(MultiChoiceStyleType::LastLine, Style::default())
}

// ---------------------------------------------------------------------------
// Grid construction
// ---------------------------------------------------------------------------

fn build_grid() -> Grid {
    // --- layout ---
    let mut grid = Grid::new(3, 2)
        .with_column_constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
        .with_row_constraints([
            Constraint::Length(3), // row 0: names
            Constraint::Length(3), // row 1: bio (grouped)
            Constraint::Length(3), // row 2: dropdowns (expand over outer border when open)
        ])
        .with_keymap(GridKeymap {
            next_cell: Some(tuirealm::event::KeyEvent {
                code: Key::Tab,
                modifiers: KeyModifiers::NONE,
            }),
            prev_cell: Some(tuirealm::event::KeyEvent {
                code: Key::BackTab,
                modifiers: KeyModifiers::SHIFT,
            }),
            ..GridKeymap::default()
        });

    // --- group the middle row ---
    grid.group_cells(CellGroup::ColSpan {
        row: 1,
        first_col: 0,
        last_col: 1,
    });

    // --- borders ---
    //
    // Outer rounded frame with a title in the top edge.
    grid.set_border(BorderPos::Grid, &BORDER_ROUNDED);
    grid.set_border_style(BorderPos::Grid, Style::default().fg(BORDER_FG));
    grid.set_border_text(BorderPos::Grid, TextAnchor::Start, 2, " New Team Member ");

    // Horizontal separator after row 0 (between names and bio).
    grid.set_border(BorderPos::AfterRow(0), &BORDER_SIMPLE);
    grid.set_border_style(BorderPos::AfterRow(0), Style::default().fg(BORDER_FG));

    // Horizontal separator after row 1 (between bio and dropdowns).
    grid.set_border(BorderPos::AfterRow(1), &BORDER_SIMPLE);
    grid.set_border_style(BorderPos::AfterRow(1), Style::default().fg(BORDER_FG));

    // Vertical column separator (suppressed automatically inside the grouped middle row).
    grid.set_border(BorderPos::AfterCol(0), &BORDER_SIMPLE);
    grid.set_border_style(BorderPos::AfterCol(0), Style::default().fg(BORDER_FG));

    // --- children ---
    grid.set_child(
        0,
        0,
        Box::new(
            TextInput::default()
                .with_title("Full Name")
                .with_placeholder("e.g. Lina Schäfer")
                .with_inactive_style(text_inactive())
                .with_active_style(text_active()),
        ),
    );
    grid.set_child(
        0,
        1,
        Box::new(
            TextInput::default()
                .with_title("Role Title")
                .with_placeholder("e.g. Senior Engineer")
                .with_inactive_style(text_inactive())
                .with_active_style(text_active()),
        ),
    );
    grid.set_child(
        1,
        0,
        Box::new(
            TextInput::default()
                .with_title("Bio / About")
                .with_placeholder("A few words about this person…")
                .with_inactive_style(text_inactive())
                .with_active_style(text_active()),
        ),
    );
    grid.set_child(
        2,
        0,
        Box::new(
            MultiChoice::default()
                .with_title("Team")
                .with_choices(TEAMS.to_vec())
                .with_placeholder("Select team")
                .with_inactive_style(mc_inactive())
                .with_active_style(mc_active())
                .with_keymap(MultiChoiceKeymap::default()),
        ),
    );
    grid.set_child(
        2,
        1,
        Box::new(
            MultiChoice::default()
                .with_title("Expertise")
                .with_choices(EXPERTISE.to_vec())
                .with_placeholder("Select skills")
                .with_inactive_style(mc_inactive())
                .with_active_style(mc_active())
                .with_keymap(MultiChoiceKeymap::default()),
        ),
    );

    grid
}

// ---------------------------------------------------------------------------
// Application types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Id {
    Form,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Msg {
    Redraw,
    Submit,

    Quit,
}

// ---------------------------------------------------------------------------
// FormComp — wraps Grid, intercepts Esc / Ctrl+S
// ---------------------------------------------------------------------------

struct FormComp {
    grid: Grid,
}

impl Component for FormComp {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        self.grid.view(frame, area);
    }
    fn query<'a>(&'a self, attr: Attribute) -> Option<QueryResult<'a>> {
        self.grid.query(attr)
    }
    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        self.grid.attr(attr, value);
    }
    fn state(&self) -> State {
        self.grid.state()
    }
    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        self.grid.perform(cmd)
    }
}

impl AppComponent<Msg, NoUserEvent> for FormComp {
    fn on(&mut self, ev: &Event<NoUserEvent>) -> Option<Msg> {
        let Event::Keyboard(key) = ev else {
            return None;
        };

        // Global shortcuts — intercepted before the grid sees them.
        match key.code {
            Key::Esc => return Some(Msg::Quit),
            Key::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Some(Msg::Submit);
            }
            _ => {}
        }

        self.grid.on_key(*key);
        Some(Msg::Redraw)
    }
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

struct Model {
    app: Application<Id, Msg, NoUserEvent>,
    quit: bool,
    redraw: bool,
    overlay: Option<String>,
    grid_ref: *mut Grid,
}

// SAFETY: single-threaded TUI application.
unsafe impl Send for Model {}

impl Model {
    fn new(mut app: Application<Id, Msg, NoUserEvent>, grid_ptr: *mut Grid) -> Self {
        app.active(&Id::Form).expect("set initial focus");
        Self {
            app,
            quit: false,
            redraw: true,
            overlay: None,
            grid_ref: grid_ptr,
        }
    }

    fn collect_result(&self) -> String {
        // SAFETY: grid_ptr is valid for the lifetime of the application loop.
        let grid = unsafe { &*self.grid_ref };

        let text = |row: usize, col: usize| -> String {
            if let State::Single(StateValue::String(s)) = grid.child_state(row, col) {
                if s.is_empty() { "—".to_string() } else { s }
            } else {
                "—".to_string()
            }
        };

        let selection = |row: usize, col: usize, labels: &[&str]| -> String {
            if let State::Vec(values) = grid.child_state(row, col) {
                let names: Vec<&str> = values
                    .iter()
                    .filter_map(|v| {
                        if let StateValue::Usize(i) = v {
                            labels.get(*i).copied()
                        } else {
                            None
                        }
                    })
                    .collect();
                if names.is_empty() {
                    "—".to_string()
                } else {
                    names.join(", ")
                }
            } else {
                "—".to_string()
            }
        };

        format!(
            "\n  Name      : {}\n  Role      : {}\n  Bio       : {}\n  Team      : {}\n  Skills    : {}\n\n  (any key to close)",
            text(0, 0),
            text(0, 1),
            text(1, 0),
            selection(2, 0, &TEAMS),
            selection(2, 1, &EXPERTISE),
        )
    }

    fn view(&mut self, frame: &mut Frame) {
        let area = frame.area();

        // Full background.
        frame.render_widget(Block::default().style(Style::default().bg(BG)), area);

        // Centred form panel.
        let panel_w = 64u16;
        let panel_h = 14u16;
        let px = area.x + area.width.saturating_sub(panel_w) / 2;
        let py = area.y + area.height.saturating_sub(panel_h) / 2;
        let panel = Rect::new(px, py, panel_w.min(area.width), panel_h.min(area.height));

        // Dark panel background (the grid's outer border paints over it).
        frame.render_widget(Block::default().style(Style::default().bg(PANEL_BG)), panel);

        self.app.view(&Id::Form, frame, panel);

        // Hint bar below the panel.
        let hint_y = panel.y + panel.height + 1;
        if hint_y < area.y + area.height {
            let hint_area = Rect::new(panel.x, hint_y, panel.width, 1);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(" Tab", Style::default().fg(ACCENT).bold()),
                    Span::styled(" next  ", Style::default().fg(HINT_FG)),
                    Span::styled("S-Tab", Style::default().fg(ACCENT).bold()),
                    Span::styled(" prev  ", Style::default().fg(HINT_FG)),
                    Span::styled("↑/↓", Style::default().fg(ACCENT).bold()),
                    Span::styled(" item  ", Style::default().fg(HINT_FG)),
                    Span::styled("Spc", Style::default().fg(ACCENT).bold()),
                    Span::styled(" toggle  ", Style::default().fg(HINT_FG)),
                    Span::styled("^S", Style::default().fg(ACCENT).bold()),
                    Span::styled(" submit  ", Style::default().fg(HINT_FG)),
                    Span::styled("Esc", Style::default().fg(ACCENT).bold()),
                    Span::styled(" quit", Style::default().fg(HINT_FG)),
                ])),
                hint_area,
            );
        }

        // Result overlay.
        if let Some(msg) = &self.overlay {
            let ow = 54u16;
            let oh = 10u16;
            let ox = area.x + area.width.saturating_sub(ow) / 2;
            let oy = area.y + area.height.saturating_sub(oh) / 2;
            let overlay = Rect::new(ox, oy, ow.min(area.width), oh.min(area.height));
            frame.render_widget(Clear, overlay);
            frame.render_widget(
                Paragraph::new(msg.as_str())
                    .block(
                        Block::bordered()
                            .title(Span::styled(
                                " ✓ Member Added ",
                                Style::default().fg(SUBMIT_BG).bold(),
                            ))
                            .border_style(Style::default().fg(SUBMIT_BG))
                            .style(Style::default().fg(INPUT_FG).bg(OVERLAY_BG)),
                    )
                    .style(Style::default().fg(INPUT_FG)),
                overlay,
            );
        }
    }
}

impl Model {
    fn update(&mut self, msg: Option<Msg>) -> Option<Msg> {
        if self.overlay.is_some() {
            // Any message while the overlay is shown → dismiss it.
            if msg.is_some() {
                self.overlay = None;
                self.redraw = true;
            }
            return None;
        }

        match msg {
            Some(Msg::Redraw) => {
                self.redraw = true;
            }
            Some(Msg::Submit) => {
                self.overlay = Some(self.collect_result());
                self.redraw = true;
            }
            Some(Msg::Quit) => {
                self.quit = true;
            }
            _ => {}
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Run loop
// ---------------------------------------------------------------------------

fn run(mut terminal: DefaultTerminal) -> std::io::Result<()> {
    execute!(std::io::stdout(), SetCursorStyle::BlinkingBar)?;

    let mut app: Application<Id, Msg, NoUserEvent> = Application::init(
        EventListenerCfg::default().crossterm_input_listener(Duration::from_millis(20), 3),
    );

    let grid = build_grid();

    // Keep a raw pointer to the Grid inside the boxed FormComp so we can read
    // child states from Model::collect_result without unsafe Arc/Mutex.
    // SAFETY: the Box is alive for the entire duration of the run loop.
    let mut form = Box::new(FormComp { grid });
    let grid_ptr: *mut Grid = &mut form.grid;

    app.mount(Id::Form, form, vec![]).expect("mount form");

    let mut model = Model::new(app, grid_ptr);

    while !model.quit {
        if model.redraw {
            terminal.draw(|f| model.view(f))?;
            model.redraw = false;
        }
        if let Ok(msgs) = model
            .app
            .tick(PollStrategy::Once(Duration::from_millis(20)))
        {
            if !msgs.is_empty() {
                model.redraw = true;
                for msg in msgs {
                    model.update(Some(msg));
                }
            }
        }
    }

    execute!(std::io::stdout(), SetCursorStyle::DefaultUserShape)?;
    Ok(())
}

fn main() -> std::io::Result<()> {
    let terminal = ratatui::init();
    let result = run(terminal);
    ratatui::restore();
    result
}
