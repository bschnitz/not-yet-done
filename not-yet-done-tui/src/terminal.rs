//! Terminal setup, teardown, and the panic safety net.
//!
//! The TUI takes over the terminal in three separate ways — raw mode, the
//! alternate screen, and the input modes of [`crate::events`] (kitty keyboard
//! disambiguation plus mouse reporting). A normal exit undoes all three. A
//! panic used to undo none of them: the process died with the alternate
//! screen still up, so the backtrace was printed onto a screen the shell then
//! discarded, and the user was left in a terminal that echoed nothing and
//! spat `<35;80;12M` at every pointer move. Recovering meant typing `reset`
//! blind.
//!
//! [`install_panic_hook`] closes that gap. It is installed by [`setup`], so
//! the invariant "the alternate screen is up ⇒ the hook is armed" cannot
//! drift apart at a call site.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Once, OnceLock};
use std::thread::ThreadId;

use anyhow::Result;
use crossterm::{
    cursor::Show,
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::events;

/// Whether the terminal is currently ours to restore. Read by the panic hook,
/// which must stay inert both before [`setup`] (a config error panicking on
/// the primary screen would otherwise emit escape noise into the shell) and
/// after [`restore`] (nothing left to undo).
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Guards against chaining the hook onto itself if `setup` ever runs twice.
static HOOK: Once = Once::new();

/// The thread that drives the render loop. A panic anywhere else — a spawned
/// content load, a watcher — must **not** tear the terminal down: only that
/// one task dies, the loop keeps drawing, and the user would be left looking
/// at a live TUI on the primary screen with raw mode off. Those panics are
/// logged and nothing more.
static OWNER: OnceLock<ThreadId> = OnceLock::new();

pub type Tui = Terminal<CrosstermBackend<io::Stdout>>;

/// Claim the terminal: raw mode, alternate screen, input modes — and arm the
/// panic hook before any of it, so a panic *during* setup is covered too.
pub fn setup() -> Result<Tui> {
    let _ = OWNER.set(std::thread::current().id());
    install_panic_hook();
    ACTIVE.store(true, Ordering::SeqCst);
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    events::resume_input_modes()?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

/// Hand the terminal back on the ordinary exit path.
pub fn restore(terminal: &mut Tui) -> Result<()> {
    disarm();
    let _ = events::suspend_input_modes();
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

/// Take the responsibility for the terminal away, reporting whether we still
/// held it. Both restore paths go through this, so a panic *inside*
/// [`restore`] cannot run the teardown a second time.
fn disarm() -> bool {
    ACTIVE.swap(false, Ordering::SeqCst)
}

/// Whether the calling thread is the one holding the terminal.
fn owns_terminal() -> bool {
    OWNER.get() == Some(&std::thread::current().id())
}

/// Best-effort teardown for the panic path.
///
/// Everything here is `let _ =`: we are already unwinding, and a terminal
/// that rejects one sequence must not stop the others from going out. The
/// order mirrors [`restore`] — input modes first (an editor or shell that
/// does not speak SGR would read the reports as garbage), then back to the
/// primary screen so the backtrace lands where the user can scroll it.
///
/// The suspend sites (external editor, interactive script) leave the
/// alternate screen while `ACTIVE` stays set. That is deliberate: they block
/// on a child process, so nothing of ours can panic in that window, and a
/// stray `LeaveAlternateScreen` on the primary screen is a no-op anyway.
fn emergency_restore() {
    if !owns_terminal() || !disarm() {
        return;
    }
    let _ = events::suspend_input_modes();
    let mut stdout = io::stdout();
    let _ = execute!(stdout, LeaveAlternateScreen, Show);
    let _ = disable_raw_mode();
    let _ = stdout.flush();
}

/// Chain a terminal-restoring hook in front of whatever hook is installed —
/// normally the default one, which prints the message and the backtrace.
/// Restoring first is the whole point: the default hook writes to stderr, and
/// on the alternate screen that output dies with the process.
pub fn install_panic_hook() {
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            emergency_restore();
            // The alternate screen has swallowed enough panics already; put
            // this one in the diagnostic log too, where the user is used to
            // looking. Best-effort, and before the default hook in case that
            // one aborts.
            not_yet_done_content::http_log::log_error("panic", &info.to_string());
            previous(info);
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disarm_reports_ownership_exactly_once() {
        // The panic hook and `restore` race on the teardown whenever a panic
        // happens during shutdown; only one of them may run it.
        assert!(!disarm(), "must be inert before setup");
        ACTIVE.store(true, Ordering::SeqCst);
        assert!(disarm(), "the first claim wins");
        assert!(!disarm(), "the second must find nothing to undo");
    }

    #[test]
    fn only_the_render_thread_tears_the_terminal_down() {
        // A panicking background load must leave the running TUI alone.
        let _ = OWNER.set(std::thread::current().id());
        assert!(owns_terminal());
        assert!(!std::thread::spawn(owns_terminal).join().unwrap());
    }
}
