mod app;
mod config;
mod events;
mod filter_builder;
mod render;
mod tabs;
mod ui;

use std::sync::Arc;

use anyhow::Result;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use shaku::HasComponent;
use std::io;

use app::App;
use config::TuiConfigService;
use not_yet_done_core::{
    config::{Config, ConfigServiceImpl, ConfigErrorKind},
    db,
    module::AppModule,
    service::TaskService,
};
use ui::theme::Theme;

#[tokio::main]
async fn main() -> Result<()> {
    let config = load_or_create_config().await?;
    let db_url = config.database.url;

    let db_conn = db::connect(&db_url, true).await?;

    let module = AppModule::builder()
        .with_component_parameters::<not_yet_done_core::repository::TaskRepositoryImpl>(
            not_yet_done_core::repository::TaskRepositoryImplParameters { db: Some(db_conn.clone()) },
        )
        .with_component_parameters::<not_yet_done_core::repository::ProjectRepositoryImpl>(
            not_yet_done_core::repository::ProjectRepositoryImplParameters { db: Some(db_conn.clone()) },
        )
        .with_component_parameters::<not_yet_done_core::repository::TagRepositoryImpl>(
            not_yet_done_core::repository::TagRepositoryImplParameters { db: Some(db_conn.clone()) },
        )
        .build();

    let task_service: Arc<dyn TaskService> = module.resolve();

    let tui_config = TuiConfigService::load()?;
    let theme      = Theme::new(tui_config.theme.clone());
    let mut app    = App::new(tui_config, theme, task_service);

    let mut terminal = setup_terminal()?;
    let result       = run_loop(&mut terminal, &mut app).await;
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

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    loop {
        app.poll_load();

        terminal.draw(|frame| render::render(frame, app))?;

        if let Some(key_str) = events::poll_event()? {
            if key_str == "ctrl+c" { break; }
            app.handle_key(&key_str);
        }

        if app.should_quit { break; }
    }
    Ok(())
}

async fn load_or_create_config() -> Result<Config> {
    if std::env::var("DATABASE_URL").is_ok() {
        return Ok(Config::default());
    }

    let service = ConfigServiceImpl::new();
    match service.get_config().await {
        Ok(config) => Ok(config),
        Err(e) if matches!(e.kind(), ConfigErrorKind::NotFound) => {
            let default = Config::default();
            save_default_config(&default)?;
            Ok(default)
        }
        Err(e) => Err(anyhow::anyhow!("Config error: {e}")),
    }
}

fn save_default_config(config: &Config) -> Result<()> {
    use std::io::Write;

    let config_dir = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?;
    let config_path = config_dir.join("not_yet_done").join("config.yaml");

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let yaml = serde_yaml::to_string(config)?;
    let mut file = std::fs::File::create(&config_path)?;
    file.write_all(yaml.as_bytes())?;

    Ok(())
}
