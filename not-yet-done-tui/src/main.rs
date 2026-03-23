mod app;
mod config;
mod events;
mod render;
mod tabs;
mod ui;

use anyhow::Result;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;

use app::App;
use config::{TuiConfigService, TuiThemeService};
use ui::theme::Theme;

#[tokio::main]
async fn main() -> Result<()> {
    let keybindings = TuiConfigService::load()?;
    let theme_cfg   = TuiThemeService::load()?;
    let theme       = Theme::new(theme_cfg);
    let mut app     = App::new(keybindings, theme);

    let mut terminal = setup_terminal()?;
    let result       = run_loop(&mut terminal, &mut app);
    restore_terminal(&mut terminal)?;

    result
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    loop {
        terminal.draw(|frame| render::render(frame, app))?;

        if let Some(key_str) = events::poll_event()? {
            // ctrl+c is always a hard exit regardless of keybinding config
            if key_str == "ctrl+c" {
                break;
            }
            if let Some(action) = app.resolve_key(&key_str) {
                app.handle_action(action);
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}
