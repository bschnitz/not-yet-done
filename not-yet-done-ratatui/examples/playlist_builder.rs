//! Example: "Playlist Builder"
//!
//! Demonstrates MultiChoice and TextInput components using a tuirealm Application
//! for event routing and focus management.
//!
//! Fields:
//!   • Genres        — multiple choice
//!   • Mood          — multiple choice
//!   • Playlist Name — text input
//!
//! Navigation: Tab / Shift+Tab.
//! In dropdowns: ↑/↓ navigate, Space toggle.
//! Enter submits (from any field). Esc exits.

use std::time::Duration;

use crossterm::{cursor::SetCursorStyle, execute};
use not_yet_done_ratatui::widgets::{
    multi_choice::{
        MultiChoice, MultiChoiceEvent, MultiChoiceKeymap, MultiChoiceStyle, MultiChoiceStyleType,
    },
    text_input::{TextInput, TextInputEvent, TextInputStyle, TextInputStyleType, ATTR_ERROR},
};
use ratatui::{
    layout::{Constraint, Direction as LayoutDir, Layout, Rect},
    prelude::Stylize,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
    DefaultTerminal, Frame,
};
use tuirealm::{
    application::PollStrategy,
    command::{Cmd, CmdResult},
    event::{Event, Key, NoUserEvent},
    Application, AttrValue, Attribute, Component, EventListenerCfg, MockComponent, State,
    StateValue, Update,
};

type App = Application<Id, Msg, NoUserEvent>;

const GENRES: [&str; 5] = ["Rock", "Jazz", "Electronic", "Hip-Hop", "Classical"];
const MOODS: [&str; 5] = ["Energetic", "Relaxing", "Melancholic", "Upbeat", "Chill"];

const BG: Color = Color::Rgb(20, 20, 30);
const PANEL_BG: Color = Color::Rgb(25, 25, 40);
const ACCENT: Color = Color::Rgb(180, 130, 255);
const INPUT_FG: Color = Color::Rgb(230, 230, 255);
const INPUT_BG: Color = Color::Rgb(35, 35, 55);
const PLACEHOLDER: Color = Color::Rgb(90, 90, 120);
const SELECTED_BG: Color = Color::Rgb(50, 50, 80);
const ACTIVE_FG: Color = Color::Rgb(255, 210, 90);
const ERROR_FG: Color = Color::Rgb(255, 100, 100);
const ACTIVE_ACCENT: Color = Color::Rgb(200, 150, 255);
const SUBMIT_FG: Color = Color::Rgb(30, 30, 50);
const SUBMIT_BG: Color = Color::Rgb(150, 200, 100);
const DIM: Color = Color::Rgb(100, 100, 130);
const OVERLAY_BG: Color = Color::Rgb(40, 40, 60);
const INACTIVE_PH: Color = Color::Rgb(45, 45, 65);

fn inactive_text_style() -> TextInputStyle {
    TextInputStyle::new()
        .prefix_color(ACCENT)
        .set_style(TextInputStyleType::Title, Style::default().fg(ACCENT))
        .set_style(TextInputStyleType::Input, Style::default().fg(INPUT_FG))
        .placeholder_color(INACTIVE_PH)
        .set_style(TextInputStyleType::Error, Style::default().fg(ERROR_FG))
}

fn active_text_style() -> TextInputStyle {
    TextInputStyle::new()
        .prefix_color(ACTIVE_ACCENT)
        .set_style(
            TextInputStyleType::Title,
            Style::default().fg(ACTIVE_ACCENT).bg(INPUT_BG),
        )
        .set_style(
            TextInputStyleType::Input,
            Style::default().fg(INPUT_FG).bg(INPUT_BG),
        )
        .placeholder_color(PLACEHOLDER)
        .set_style(TextInputStyleType::Error, Style::default().fg(ERROR_FG))
}

fn inactive_mc_style() -> MultiChoiceStyle {
    MultiChoiceStyle::new()
        .prefix_color(ACCENT)
        .set_style(MultiChoiceStyleType::Title, Style::default().fg(ACCENT))
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

fn active_mc_style() -> MultiChoiceStyle {
    MultiChoiceStyle::new()
        .prefix_color(ACTIVE_ACCENT)
        .set_style(
            MultiChoiceStyleType::Title,
            Style::default().fg(ACTIVE_ACCENT).bg(INPUT_BG),
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Id {
    Genres,
    Mood,
    PlaylistName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Msg {
    FocusNext,
    FocusPrev,
    GenresChanged,
    MoodChanged,
    PlaylistNameChanged(String),
    Submit,
    Quit,
    Redraw,
}

// --- Generic MultiChoice wrapper using a trait ---

/// Generic wrapper around MultiChoice. The type parameter H determines which
/// Msg is emitted when the selection changes.
struct MultiChoiceWrapper {
    component: MultiChoice,
    changed_msg: fn() -> Msg,
    /// Mirrors the component's open/closed state so the wrapper can toggle on Enter.
    is_open: bool,
}

// Manual MockComponent impl — forwards everything to the inner component but
// intercepts Focus changes so we can keep `is_open` in sync.
impl MockComponent for MultiChoiceWrapper {
    fn view(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        self.component.view(frame, area)
    }
    fn query(&self, attr: Attribute) -> Option<AttrValue> {
        self.component.query(attr)
    }
    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        if attr == Attribute::Focus {
            if let AttrValue::Flag(f) = value {
                self.is_open = f; // component opens on focus gain, closes on focus loss
            }
        }
        self.component.attr(attr, value)
    }
    fn state(&self) -> State {
        self.component.state()
    }
    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        self.component.perform(cmd)
    }
}

impl Component<Msg, NoUserEvent> for MultiChoiceWrapper {
    fn on(&mut self, ev: Event<NoUserEvent>) -> Option<Msg> {
        if let Event::Keyboard(key_ev) = ev {
            match key_ev.code {
                Key::Tab => return Some(Msg::FocusNext),
                Key::BackTab => return Some(Msg::FocusPrev),
                Key::Esc => return Some(Msg::Quit),
                Key::Enter => {
                    if self.is_open {
                        self.component.perform(Cmd::Cancel);
                        self.is_open = false;
                    } else {
                        self.component.perform(Cmd::Submit);
                        self.is_open = true;
                    }
                    return Some(Msg::Redraw);
                }
                _ => {}
            }
            return match self.component.on(Event::Keyboard(key_ev)) {
                Some(MultiChoiceEvent::SelectionChanged(_)) => Some((self.changed_msg)()),
                Some(MultiChoiceEvent::Closed) => {
                    self.is_open = false;
                    Some(Msg::Redraw)
                }
                Some(_) => Some(Msg::Redraw),
                None => None,
            };
        }
        None
    }
}

// --- PlaylistName component (unchanged) ---

#[derive(MockComponent)]
struct PlaylistNameComp {
    component: TextInput,
}

impl Component<Msg, NoUserEvent> for PlaylistNameComp {
    fn on(&mut self, ev: Event<NoUserEvent>) -> Option<Msg> {
        if let Event::Keyboard(key_ev) = ev {
            match key_ev.code {
                Key::Tab => return Some(Msg::FocusNext),
                Key::BackTab => return Some(Msg::FocusPrev),
                Key::Esc => return Some(Msg::Quit),
                _ => {}
            }
            return match self.component.on(Event::Keyboard(key_ev)) {
                Some(TextInputEvent::Changed(s)) => Some(Msg::PlaylistNameChanged(s)),
                Some(TextInputEvent::Submitted(_)) => Some(Msg::Submit),
                None => None,
            };
        }
        None
    }
}

// --- Builder functions ---

fn make_genres() -> MultiChoiceWrapper {
    fn changed() -> Msg { Msg::GenresChanged }
    MultiChoiceWrapper {
        component: MultiChoice::default()
            .with_title("Genres")
            .with_choices(GENRES.to_vec())
            .with_placeholder("Select genres")
            .with_inactive_style(inactive_mc_style())
            .with_active_style(active_mc_style())
            .with_keymap(MultiChoiceKeymap::default()),
        changed_msg: changed,
        is_open: false,
    }
}

fn make_mood() -> MultiChoiceWrapper {
    fn changed() -> Msg { Msg::MoodChanged }
    MultiChoiceWrapper {
        component: MultiChoice::default()
            .with_title("Mood")
            .with_choices(MOODS.to_vec())
            .with_placeholder("Select mood")
            .with_inactive_style(inactive_mc_style())
            .with_active_style(active_mc_style())
            .with_keymap(MultiChoiceKeymap::default()),
        changed_msg: changed,
        is_open: false,
    }
}

fn make_playlist_name() -> PlaylistNameComp {
    PlaylistNameComp {
        component: TextInput::default()
            .with_title("Playlist Name")
            .with_placeholder("e.g. 'Evening Chill'")
            .with_inactive_style(inactive_text_style())
            .with_active_style(active_text_style()),
    }
}

// --- Rest of the application (unchanged beyond the above) ---

const FOCUS_ORDER: [Id; 3] = [Id::Genres, Id::Mood, Id::PlaylistName];

struct Model {
    app: App,
    quit: bool,
    redraw: bool,
    active: Id,
    submitted: Option<String>,
}

impl Model {
    fn new(app: App) -> Self {
        Self {
            app,
            quit: false,
            redraw: true,
            active: Id::Genres,
            submitted: None,
        }
    }

    fn focus_next(&self) -> Id {
        let idx = FOCUS_ORDER.iter().position(|id| *id == self.active).unwrap_or(0);
        FOCUS_ORDER[(idx + 1) % FOCUS_ORDER.len()].clone()
    }

    fn focus_prev(&self) -> Id {
        let idx = FOCUS_ORDER.iter().position(|id| *id == self.active).unwrap_or(0);
        FOCUS_ORDER[(idx + FOCUS_ORDER.len() - 1) % FOCUS_ORDER.len()].clone()
    }

    fn set_focus(&mut self, id: Id) {
        self.active = id.clone();
        self.app.active(&id).expect("failed to set focus");
    }

    fn is_valid(&self) -> bool {
        let mc_selected = |id: &Id| {
            if let Ok(State::Vec(v)) = self.app.state(id) { !v.is_empty() } else { false }
        };
        let text_non_empty = |id: &Id| {
            if let Ok(State::One(StateValue::String(s))) = self.app.state(id) {
                !s.is_empty()
            } else {
                false
            }
        };
        mc_selected(&Id::Genres)
            && mc_selected(&Id::Mood)
            && text_non_empty(&Id::PlaylistName)
    }

    fn validate(&mut self) -> bool {
        let name_ok =
            if let Ok(State::One(StateValue::String(s))) = self.app.state(&Id::PlaylistName) {
                !s.is_empty()
            } else {
                false
            };

        if name_ok {
            self.app
                .attr(
                    &Id::PlaylistName,
                    Attribute::Custom(ATTR_ERROR),
                    AttrValue::Flag(false),
                )
                .expect("failed to clear error attr");
        } else {
            self.app
                .attr(
                    &Id::PlaylistName,
                    Attribute::Custom(ATTR_ERROR),
                    AttrValue::String("Playlist name required".into()),
                )
                .expect("failed to set error attr");
        }

        self.is_valid()
    }

    fn collect_result(&self) -> Option<String> {
        let indices_from_state = |id: &Id| -> Vec<usize> {
            if let Ok(State::Vec(values)) = self.app.state(id) {
                values
                    .into_iter()
                    .filter_map(|v| if let StateValue::Usize(i) = v { Some(i) } else { None })
                    .collect()
            } else {
                vec![]
            }
        };

        let genre_names = indices_from_state(&Id::Genres)
            .into_iter()
            .filter_map(|i| GENRES.get(i).copied())
            .collect::<Vec<_>>()
            .join(", ");

        let mood_names = indices_from_state(&Id::Mood)
            .into_iter()
            .filter_map(|i| MOODS.get(i).copied())
            .collect::<Vec<_>>()
            .join(", ");

        let playlist_name =
            if let Ok(State::One(StateValue::String(s))) = self.app.state(&Id::PlaylistName) {
                s
            } else {
                return None;
            };

        if genre_names.is_empty() || mood_names.is_empty() || playlist_name.is_empty() {
            return None;
        }

        Some(format!(
            "\n  Playlist  : {}\n  Genres    : {}\n  Mood      : {}\n\n  (any key closes)",
            playlist_name, genre_names, mood_names,
        ))
    }

    fn view(&mut self, frame: &mut Frame) {
        let area = frame.area();

        frame.render_widget(Block::default().style(Style::default().bg(BG)), area);

        let panel_w = 60u16;
        let panel_h = 18u16;
        let px = area.x + area.width.saturating_sub(panel_w) / 2;
        let py = area.y + area.height.saturating_sub(panel_h) / 2;
        let panel = Rect::new(
            px,
            py,
            panel_w.min(area.width),
            panel_h.min(area.height),
        );
        frame.render_widget(Block::default().style(Style::default().bg(PANEL_BG)), panel);

        let inner = Rect::new(
            panel.x + 2,
            panel.y + 1,
            panel.width.saturating_sub(4),
            panel.height.saturating_sub(2),
        );

        let chunks = Layout::default()
            .direction(LayoutDir::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(inner);

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("♪ ", Style::default().fg(ACTIVE_ACCENT)),
                Span::styled(
                    "Playlist Builder",
                    Style::default().fg(ACCENT).bold(),
                ),
            ])),
            chunks[0],
        );

        let field_areas = [
            (Id::Genres, chunks[2]),
            (Id::Mood, chunks[4]),
            (Id::PlaylistName, chunks[6]),
        ];
        for (id, area) in &field_areas {
            if *id != self.active {
                self.app.view(id, frame, *area);
            }
        }
        for (id, area) in &field_areas {
            if *id == self.active {
                self.app.view(id, frame, *area);
            }
        }

        let valid = self.is_valid();
        let submit_span = if valid {
            Span::styled(
                " [ Enter → Create Playlist ] ",
                Style::default().fg(SUBMIT_FG).bg(SUBMIT_BG).bold(),
            )
        } else {
            Span::styled(
                " [ Enter → Create Playlist ] ",
                Style::default().fg(Color::Gray).bg(Color::DarkGray),
            )
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![submit_span])),
            chunks[8],
        );

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" Tab", Style::default().fg(ACCENT).bold()),
                Span::styled(" next  ", Style::default().fg(DIM)),
                Span::styled("S-Tab", Style::default().fg(ACCENT).bold()),
                Span::styled(" prev  ", Style::default().fg(DIM)),
                Span::styled("↑/↓", Style::default().fg(ACCENT).bold()),
                Span::styled(" item  ", Style::default().fg(DIM)),
                Span::styled("Spc", Style::default().fg(ACCENT).bold()),
                Span::styled(" select  ", Style::default().fg(DIM)),
                Span::styled("Esc", Style::default().fg(ACCENT).bold()),
                Span::styled(" quit", Style::default().fg(DIM)),
            ])),
            chunks[10],
        );

        if let Some(msg) = &self.submitted {
            let ow = 50u16;
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
                                " ✓ Playlist Created ",
                                Style::default().fg(ACTIVE_ACCENT).bold(),
                            ))
                            .style(Style::default().fg(INPUT_FG).bg(OVERLAY_BG)),
                    )
                    .style(Style::default().fg(INPUT_FG)),
                overlay,
            );
        }
    }
}

impl Update<Msg> for Model {
    fn update(&mut self, msg: Option<Msg>) -> Option<Msg> {
        if self.submitted.is_some() {
            if msg.is_some() {
                self.submitted = None;
                self.redraw = true;
            }
            return None;
        }

        match msg {
            Some(Msg::FocusNext) => {
                self.set_focus(self.focus_next());
                self.redraw = true;
            }
            Some(Msg::FocusPrev) => {
                self.set_focus(self.focus_prev());
                self.redraw = true;
            }
            Some(Msg::GenresChanged) | Some(Msg::MoodChanged) => {
                self.redraw = true;
            }
            Some(Msg::PlaylistNameChanged(_)) => {
                self.validate();
                self.redraw = true;
            }
            Some(Msg::Submit) => {
                let valid = self.validate();
                if valid {
                    self.submitted = self.collect_result();
                }
                self.redraw = true;
            }
            Some(Msg::Quit) => {
                self.quit = true;
            }
            Some(Msg::Redraw) => {
                self.redraw = true;
            }
            _ => {}
        }
        None
    }
}

fn run(mut terminal: DefaultTerminal) -> std::io::Result<()> {
    execute!(std::io::stdout(), SetCursorStyle::BlinkingBar)?;

    let mut app: App = Application::init(
        EventListenerCfg::default()
            .crossterm_input_listener(Duration::from_millis(20), 3),
    );

    app.mount(Id::Genres, Box::new(make_genres()), vec![])
        .expect("failed to mount Genres");
    app.mount(Id::Mood, Box::new(make_mood()), vec![])
        .expect("failed to mount Mood");
    app.mount(Id::PlaylistName, Box::new(make_playlist_name()), vec![])
        .expect("failed to mount PlaylistName");

    app.active(&Id::Genres).expect("failed to set initial focus");

    let mut model = Model::new(app);

    while !model.quit {
        if model.redraw {
            terminal.draw(|f| model.view(f))?;
            model.redraw = false;
        }

        if let Ok(msgs) = model.app.tick(PollStrategy::Once) {
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
