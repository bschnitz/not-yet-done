mod action;
mod active_surface;
mod app;
mod components;
mod config;
mod edit_session;
mod events;
mod keymap;
mod query_filter;

mod render;
mod tabs;
mod ui;
pub mod views;

use std::sync::Arc;

use anyhow::Result;
use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use shaku::HasComponent;
use std::io;

use app::{App, EditorRequest};
use config::TuiConfigService;
use not_yet_done_core::{
    config::{Config, ConfigErrorKind, ConfigServiceImpl},
    db,
    module::CoreModule,
};
use tabs::Tab;
use ui::theme::Theme;

#[tokio::main]
async fn main() -> Result<()> {
    let config = load_or_create_config().await?;

    // Install diagnostic logging before anything can log. Errors (including
    // everything surfaced via `notify_error`) go to a rotating daily file;
    // env vars (NYD_DEBUG, NYD_LOG_DIR, …) still override the config.
    not_yet_done_content::http_log::configure(not_yet_done_content::http_log::LogSettings {
        enabled: config.logging.enabled,
        directory: config.logging.directory.clone(),
        retention_days: config.logging.retention_days as i64,
        verbose: config.logging.verbose,
    });

    let db_url = config.database.url;

    let db_conn = db::connect(&db_url, true).await?;

    // The app shell's own Shaku module (C3/C5 of the DB-split). The task
    // domain no longer lives here: Tasks/Trackings are self-contained content
    // adapters that open their own database, and the tag feature is now fully
    // adapter-driven (C5c), so the TUI binary no longer depends on
    // `not-yet-done-task-core` at all. This DB (`config.database.url`) backs
    // only the app shell's settings / saved queries / query shortcuts / links.
    let core_module = CoreModule::builder()
        .with_component_parameters::<not_yet_done_core::repository::SettingsRepositoryImpl>(
            not_yet_done_core::repository::SettingsRepositoryImplParameters {
                db: Some(db_conn.clone()),
            },
        )
        .with_component_parameters::<not_yet_done_core::repository::QueryShortcutRepositoryImpl>(
            not_yet_done_core::repository::QueryShortcutRepositoryImplParameters {
                db: Some(db_conn.clone()),
            },
        )
        .with_component_parameters::<not_yet_done_core::repository::LinkRepositoryImpl>(
            not_yet_done_core::repository::LinkRepositoryImplParameters {
                db: Some(db_conn.clone()),
            },
        )
        .build();

    let query_shortcut_repo: Arc<dyn not_yet_done_core::repository::QueryShortcutRepository> =
        core_module.resolve();
    let settings_repo: Arc<dyn not_yet_done_core::repository::SettingsRepository> =
        core_module.resolve();
    let link_repo: Arc<dyn not_yet_done_core::repository::LinkRepository> = core_module.resolve();
    let tui_config = TuiConfigService::load()?;
    let theme = Theme::new(tui_config.theme.clone());
    // Kept out of the move into `App::new`: the terminal can only be asked
    // about its graphics support once the alternate screen is up, which is
    // well after the config is read.
    let images_cfg = tui_config.images.clone();

    // Host-owned cross-adapter event bus (Phase C4): the App owns it and
    // hands every adapter a `HostContext` at construction. The self-contained
    // local Tasks/Trackings adapters coordinate over it — keyed by their DSN,
    // so two tabs on the same database repaint each other — while remote
    // adapters ignore it. Built by `not-yet-done-host` so the CLI and Waybar
    // construct the identical context (Block D).
    let host_ctx = not_yet_done_host::host_context();

    // Adapter factories are now stateless (each local factory opens its own
    // database in `create`), so this is just the bare builder fn — still a
    // boxed closure so `App::reload_config` can rebuild the same set. The set
    // itself lives in `not-yet-done-host` so every front-end shares one
    // registry.
    let factory_builder: Box<
        dyn Fn() -> std::collections::HashMap<String, Box<dyn not_yet_done_content::AdapterFactory>>
            + Send
            + Sync,
    > = Box::new(not_yet_done_host::factories);

    let mut app = App::new(
        tui_config,
        theme,
        query_shortcut_repo,
        settings_repo,
        link_repo,
        factory_builder,
        host_ctx,
    );

    // Wire every content view: DB-persisted state (column config, saved
    // queries, default query, sort spec), the jump alphabet, the adapter
    // watchers and the initial fetch — in that order per view, so the first
    // fetch already uses the stamped default query. The config reloads run
    // the same routine on the views they rebuild. (Tracking state is no
    // longer cached at App level — the action-bar highlight reads it live
    // from each adapter.)
    app.wire_content_views();

    // Daily backup of the legacy core DB (`nyd.db`: saved queries, settings,
    // links), owned by core. Best-effort — a backup failure must never block
    // startup.
    if let Err(e) = not_yet_done_core::service::BackupServiceImpl
        .ensure_daily_backup()
        .await
    {
        eprintln!("Backup warning (nyd.db): {e}");
    }
    // Fire the `connected` lifecycle hook for every configured instance that
    // declares one (D5). This generalises the old hard-coded tasks.db daily
    // backup: the shipped `tasks.yaml` binds the `backup` action to `connected`
    // with a 24h throttle, so the task DB is backed up once a day on first
    // launch — but now any adapter can hook any action on any cadence, with no
    // TUI code change. The throttle is checked before each adapter is built, so
    // a within-window launch constructs nothing extra.
    not_yet_done_host::fire_connected_hooks().await;

    let mut terminal = setup_terminal()?;
    // Ask the terminal which graphics protocol it speaks. Must sit between
    // entering the alternate screen and starting the event reader: the query
    // writes an escape sequence and reads the reply straight off stdin, and a
    // running reader would swallow it. Panes bind to the answer lazily, on
    // their first markdown render, so the ones built above still see it.
    views::images::init_terminal_graphics(images_cfg.enabled, images_cfg.max_height);
    let result = run_loop(&mut terminal, &mut app).await;
    restore_terminal(&mut terminal)?;

    result
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    events::enable_kitty_protocol()?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let _ = events::disable_kitty_protocol();
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    // Event-driven (1b) render loop. Idle = parked in `select!` on
    // terminal input plus the async result/commit channels, so a parked
    // TUI costs ~0 % CPU and an out-of-band message (a background load
    // finishing, or — once wired — an adapter pushing a live invalidation)
    // wakes the loop the instant it arrives, with no 200 ms poll deadline.
    //
    // Two change-source families exist:
    //   * waker-backed — `load_rx`, `commit_rx`, terminal input — sit
    //     directly in the `select!` and wake it on arrival.
    //   * poll-backed — the detached editor (`:w` live-reload, `.done`
    //     close), the detached script (completion marker), the `Busy`
    //     banner second counter and active-tracking duration cells — have
    //     no waker. A periodic ticker services them, but it is armed *only
    //     while one of them is pending* (`App::needs_periodic_tick`); when
    //     nothing time-based lives, its branch is disabled and the loop
    //     stays parked.
    //
    // `sync_components` + `terminal.draw` still run only when an iteration
    // touched visible state (`dirty`). Rationale and the prior 1a
    // (200 ms dirty-gated poll) loop are in
    // docs/decisions/0001-render-loop-dirty-gating.md.
    use crossterm::event::{Event, EventStream};
    use tokio_stream::StreamExt;

    // crossterm's `EventStream` runs a background thread that blocks on
    // stdin. It must be torn down before any child process (editor,
    // script) takes over the terminal — otherwise that thread steals the
    // child's input — and recreated afterwards. `reader` is therefore
    // dropped+rebuilt around every suspend point below.
    let mut reader = EventStream::new();
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(200));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut dirty = true; // always paint the first frame
    loop {
        // Inline pictures a markdown body asked for during the last build.
        // A no-op walk when nothing is queued; each hit spawns a download
        // that reports back through `load_rx`.
        app.pump_image_downloads();

        if dirty {
            app.sync_components();
            terminal.draw(|frame| render::render(frame, app))?;
            dirty = false;

            // Tables fit their columns to the pane width recorded during the
            // draw above. On first paint, a terminal resize, or a preview
            // toggle that width just changed, so re-fit the affected tables
            // and draw once more. Converges immediately — the next re-fit
            // pass is a no-op once widths match — so the loop still parks on
            // the select! below.
            if app.refit_visible_tables() {
                dirty = true;
                continue;
            }
        }

        // Editor requests deferred from an async `LoadMsg` drain — e.g. a
        // `NodeActionDispatched` carrying `ActionDispatch::OpenEditor` (the
        // DB-Script `e` shortcut path). The async path stashes its
        // EditorRequest in `app.pending_editor_request` because
        // `handle_load_msg` can't bubble one out. Suspends the terminal,
        // so tear the reader down for the duration.
        if let Some(req) = app.pending_editor_request.take() {
            if req.suspends_terminal() {
                drop(reader);
                dispatch_editor_request(terminal, app, req).await?;
                reader = EventStream::new();
            } else {
                dispatch_editor_request(terminal, app, req).await?;
            }
            dirty = true;
            continue;
        }

        if app.should_quit {
            break;
        }

        // Arm the periodic ticker only while a poll-backed source is live.
        let periodic = app.needs_periodic_tick();

        // Deadline at which a half-typed chord's which-key preview pops up.
        // `None` disarms the branch (parks on a never-resolving future).
        let which_key_deadline = app.which_key_deadline();

        tokio::select! {
            // Background-loaded results (tasks, content items, adapter
            // status, …). `recv()` consumes one; handle it, then drain any
            // siblings that landed in the same wake.
            Some(msg) = app.load_rx.recv() => {
                dirty |= app.handle_load_msg(msg);
                dirty |= app.poll_load();
            }

            // Background session-commit results. A `Reopen` outcome
            // relaunches the editor with the validation-error buffer, which
            // suspends the terminal (tear the reader down for it).
            Some(msg) = app.commit_rx.recv() => {
                if let Some(error_content) = app.handle_commit_msg(msg).await {
                    drop(reader);
                    reopen_editor_with_errors(terminal, app, &error_content)?;
                    reader = EventStream::new();
                }
                dirty = true;
            }

            // Which-key preview reveal: fires once the pending chord has
            // sat for `which_key.delay_ms`. Disarmed by parking on a
            // never-resolving future when no deadline is set.
            _ = async {
                match which_key_deadline {
                    Some(d) => tokio::time::sleep_until(tokio::time::Instant::from_std(d)).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                dirty |= app.reveal_which_key();
            }

            // Poll-backed change sources, serviced only while one is
            // pending. Each self-gates on its own interval/condition, so
            // running them all every tick is cheap.
            _ = ticker.tick(), if periodic => {
                dirty |= app.tick_animations();
                dirty |= app.poll_live_editor().await;
                dirty |= app.poll_editor_close();
                dirty |= app.poll_detached_script();
            }

            // Terminal input. A key resolving to an `EditorRequest`
            // suspends the terminal (tear the reader down for it); a resize
            // just forces a repaint.
            maybe_event = reader.next() => {
                match maybe_event {
                    Some(Ok(event)) => {
                        if let Some(key_str) = events::event_to_key_string(&event) {
                            let req = app.handle_key(&key_str);
                            // Sync the which-key preview to the (possibly
                            // changed) pending chord: close, arm the reveal
                            // timer, or narrow it live.
                            app.reconcile_which_key();
                            if req.suspends_terminal() {
                                drop(reader);
                                dispatch_editor_request(terminal, app, req).await?;
                                reader = EventStream::new();
                            } else if !matches!(req, EditorRequest::None) {
                                dispatch_editor_request(terminal, app, req).await?;
                            }
                            dirty = true;
                        } else if matches!(event, Event::Resize(_, _)) {
                            dirty = true;
                        }
                    }
                    // Transient decode error: ignore, keep the reader.
                    Some(Err(_)) => {}
                    // Stream ended (stdin closed): rebuild and carry on.
                    None => { reader = EventStream::new(); }
                }
            }
        }
    }
    Ok(())
}

/// Dispatch a single [`EditorRequest`]. Shared by the keypress branch
/// and the async-load-drain branch so a `NodeActionDispatched` that
/// resolves to `OpenEditor` runs through the exact same launch logic
/// as a direct keypress.
async fn dispatch_editor_request(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    req: EditorRequest,
) -> Result<()> {
    match req {
        EditorRequest::Inline {
            command,
            content,
            suffix,
            spawn_context,
        } => {
            let mut editor_content = content;
            loop {
                let result = run_inline_editor_get_content(
                    terminal,
                    &command,
                    &editor_content,
                    suffix,
                    &spawn_context,
                )?;
                let Some(result_content) = result else { break };
                match app.process_editor_content(&result_content).await {
                    Some(error_content) => {
                        editor_content = error_content;
                    }
                    None => break,
                }
            }
        }
        EditorRequest::Launch {
            command,
            content,
            suffix,
            spawn_context,
        } => {
            run_launch_editor(terminal, app, &command, &content, suffix, &spawn_context)?;
        }
        EditorRequest::Script {
            script_path,
            stdin_json,
            capture,
            output_suffix,
            child_env,
        } => {
            run_interactive_script(
                terminal,
                app,
                &script_path,
                &stdin_json,
                capture,
                &output_suffix,
                &child_env,
            )?;
        }
        // A `:w` in the builtin editor pane: no child process, no terminal
        // hand-over — just the session's async `live_apply`.
        EditorRequest::BuiltinLiveApply { content } => {
            app.apply_builtin_live_save(&content).await;
        }
        EditorRequest::None => {}
    }
    Ok(())
}

/// Clear the screen and force a full redraw on the next `draw`, **without**
/// querying the cursor position.
///
/// We must not use [`Terminal::clear`] on the post-editor resume path: since
/// ratatui 0.30 (ratatui-core 0.1.1) `clear()` snapshots the cursor via
/// `get_cursor_position()`, which writes `ESC[6n` and blocks reading the
/// terminal's reply from stdin. crossterm's `EventStream` reader thread is
/// dropped before the editor runs, but dropping it does not synchronously
/// join the thread — a lingering blocking `read()` can swallow the `ESC[6n`
/// reply, so `position()` times out with "The cursor position could not be
/// read within a normal duration" and the whole TUI aborts.
///
/// For a fullscreen viewport, `resize` to the current size has the same
/// visible effect as the old `clear()` (issues `ESC[2J` and resets the back
/// buffer so the next frame is a full repaint) but never touches the cursor.
fn force_full_redraw(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let size = terminal.size()?;
    terminal.resize(ratatui::layout::Rect::new(0, 0, size.width, size.height))?;
    Ok(())
}

/// Pause ratatui, run the editor inline (blocking), then restore.
/// Returns the edited content, or None if the editor failed.
fn run_inline_editor_get_content(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    command: &str,
    content: &str,
    suffix: &str,
    spawn_context: &crate::edit_session::EditorSpawnContext,
) -> Result<Option<String>> {
    let cmd = if command.is_empty() {
        None
    } else {
        Some(command)
    };
    let _ = events::disable_kitty_protocol();
    let result = match not_yet_done_ratatui::open_editor_inline_in(
        cmd,
        content,
        Some(suffix),
        spawn_context.tempfile_dir.as_deref(),
        spawn_context.tempfile_prefix,
        &spawn_context.child_env,
        spawn_context.persistent_file.as_deref(),
    ) {
        Ok(content) => Some(content),
        Err(e) => {
            eprintln!("Editor error: {e}");
            None
        }
    };
    let _ = events::enable_kitty_protocol();
    force_full_redraw(terminal)?;
    Ok(result)
}

/// Re-open editor with validation error content.
fn reopen_editor_with_errors(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    error_content: &str,
) -> Result<()> {
    // Resolve the SAME editor profile the original open used, via the
    // still-pending session's `editor_profile()` (None → `default`).
    let editor = app.config.editors.resolve(
        app.pending_session
            .as_ref()
            .and_then(|s| s.editor_profile()),
    );
    let cmd = editor.command.clone();
    let pause_tui = editor.pause_tui;
    let command = if cmd.is_empty() {
        None
    } else {
        Some(cmd.as_str())
    };
    // Pull suffix + spawn context from the still-pending session. The
    // session's `spawn_context()` is recomputed (not the original
    // snapshot from `open_session`) so a connection that came up since
    // can populate `child_env` on the reopen — matches the existing
    // tempfile_dir behaviour, which used to be re-queried here too.
    let (suffix, spawn_context) = match app.pending_session.as_ref() {
        Some(s) => (s.suffix().to_string(), s.spawn_context()),
        None => (".md".to_string(), Default::default()),
    };
    if pause_tui {
        let _ = events::disable_kitty_protocol();
        if let Ok(handle) = not_yet_done_ratatui::open_editor_launch_in(
            command,
            error_content,
            Some(&suffix),
            spawn_context.tempfile_dir.as_deref(),
            spawn_context.tempfile_prefix,
            &spawn_context.child_env,
            spawn_context.persistent_file.as_deref(),
        ) {
            app.detached_editor = Some(handle);
        }
        let _ = events::enable_kitty_protocol();
    }
    force_full_redraw(terminal)?;
    Ok(())
}

/// Pause ratatui briefly, launch the editor command (returns immediately),
/// resume ratatui, and store the handle for `.done` polling.
fn run_launch_editor(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    command: &str,
    content: &str,
    suffix: &str,
    spawn_context: &crate::edit_session::EditorSpawnContext,
) -> Result<()> {
    let cmd = if command.is_empty() {
        None
    } else {
        Some(command)
    };
    let _ = events::disable_kitty_protocol();
    match not_yet_done_ratatui::open_editor_launch_in(
        cmd,
        content,
        Some(suffix),
        spawn_context.tempfile_dir.as_deref(),
        spawn_context.tempfile_prefix,
        &spawn_context.child_env,
        spawn_context.persistent_file.as_deref(),
    ) {
        Ok(handle) => {
            app.detached_editor = Some(handle);
        }
        Err(e) => {
            eprintln!("Editor launch error: {e}");
        }
    }
    let _ = events::enable_kitty_protocol();
    force_full_redraw(terminal)?;
    Ok(())
}

/// Pause TUI, run a script with full terminal control, then resume.
/// If `capture` is true, stdout/stderr are captured and shown in an editor.
fn run_interactive_script(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    script_path: &str,
    stdin_json: &str,
    capture: bool,
    output_suffix: &str,
    child_env: &std::collections::HashMap<String, String>,
) -> Result<()> {
    use crossterm::event::{self as ct_event, Event};
    use std::process::{Command, Stdio};

    // Leave alternate screen so script gets the real terminal.
    let _ = events::disable_kitty_protocol();
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen
    )?;

    let path = std::path::Path::new(script_path);
    let cwd = path.parent().unwrap_or(std::path::Path::new("."));

    // Write JSON to temp file.
    let json_path =
        std::env::temp_dir().join(format!("nyd-interactive-{}.json", std::process::id()));
    std::fs::write(&json_path, stdin_json)?;

    let result = if capture {
        // Interactive + capture: stderr to terminal, stdout piped.
        Command::new(script_path)
            .arg(&json_path)
            .current_dir(cwd)
            .envs(child_env)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?
            .wait_with_output()
    } else {
        // Pure interactive: full terminal.
        Command::new(script_path)
            .arg(&json_path)
            .current_dir(cwd)
            .envs(child_env)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?
            .wait_with_output()
    };

    // Wait for keypress before returning to TUI.
    println!("\nPress any key to return to not-yet-done...");
    enable_raw_mode()?;
    loop {
        if ct_event::poll(std::time::Duration::from_millis(500))? {
            if let Event::Key(_) = ct_event::read()? {
                break;
            }
        }
    }
    disable_raw_mode()?;

    // Restore TUI.
    execute!(
        terminal.backend_mut(),
        crossterm::terminal::EnterAlternateScreen
    )?;
    enable_raw_mode()?;
    let _ = events::enable_kitty_protocol();
    force_full_redraw(terminal)?;

    // Handle output.
    match result {
        Ok(output) => {
            if capture {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if !stdout.trim().is_empty() {
                    let session = edit_session::ScriptOutputSession::new(stdout.to_string())
                        .with_suffix(output_suffix);
                    let _ = app.open_session(Box::new(session));
                } else {
                    app.notify("Script finished (no captured output)".to_string());
                }
            } else if !output.status.success() {
                app.notify_error(format!("Script exited with {}", output.status));
            } else {
                app.notify("Script finished".to_string());
            }
        }
        Err(e) => {
            app.notify_error(format!("Script error: {e}"));
        }
    }

    // Reload the active content view's current level (the script may have
    // mutated the underlying data).
    let Tab::Content(idx) = app.active_tab;
    if let Some(pane_id) = app.content_view(idx).map(|cv| cv.active_pane_id()) {
        app.reload_content_pane_current_level(idx, pane_id);
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
