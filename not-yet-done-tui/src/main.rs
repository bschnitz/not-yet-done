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
use config::TuiConfigService;
use ui::theme::Theme;

#[tokio::main]
async fn main() -> Result<()> {
    let tui_config = TuiConfigService::load()?;
    let theme      = Theme::new(tui_config.theme.clone());
    let mut app    = App::new(tui_config, theme);

    let mut terminal = setup_terminal()?;
    let result       = run_loop(&mut terminal, &mut app);
    restore_terminal(&mut terminal)?;

    result
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
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
            if key_str == "ctrl+c" { break; }
            app.handle_key(&key_str);
        }

        if app.should_quit { break; }
    }
    Ok(())
}
