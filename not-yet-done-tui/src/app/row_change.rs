//! The `row_change` script hook: run a view script when the cursor lands on
//! another row.
//!
//! The vocabulary lives with the other hooks in
//! [`crate::app::script_hook`]; what is here is everything this one hook
//! needs and the other two do not, because it is the only one that fires
//! from a **key press** instead of from a finished load:
//!
//!   - **Detection** ([`App::note_row_cursor`]) — called once per main-loop
//!     turn. It compares the focused pane's selected row against the row
//!     that pane last *reported* and arms a timer when they differ. The
//!     comparison is on the row's node id, so a reload that keeps the cursor
//!     where it was, or a sort that carries the same row to another index,
//!     is not a row change.
//!   - **A settle delay** (`script.row_change_delay_ms`) — the timer is
//!     re-armed on every further change, so holding `j` through forty rows
//!     runs the script once, for row forty. `previous_index` is then the row
//!     the burst started from: the rows in between were never reported to
//!     anybody.
//!   - **A detached run** ([`App::fire_row_change_hooks`]) — the child is
//!     spawned and not waited for, and its output goes to a file rather than
//!     to the terminal the TUI is drawing on. [`App::poll_row_change_runs`]
//!     reaps it on the existing ticker.
//!
//! **Why it may answer with nothing.** A `reload` hook may hand back
//! commands; a command that reloads or jumps moves the cursor, which fires
//! this hook again, and the depth guard that stops a `reload` loop rides on
//! the *load* — which a row change does not have. So a script bound here
//! acts outside the TUI and is refused at binding time if its `# mode:`
//! header emits commands.
//!
//! **One run per pane at a time.** A cursor that moves on while a run is
//! still alive re-arms the timer instead of starting a second child: two
//! renders never race for the same output file, and the script converges on
//! the row the cursor actually came to rest on.

use std::collections::HashMap;
use std::time::Instant;

use crate::app::App;
use crate::app::script_hook::{ScriptHook, hook_binding_rejection};
use crate::tabs::Tab;
use crate::views::content_view::PaneId;

/// Which pane a mark, a run and a binding belong to.
type PaneKey = (usize, PaneId);

/// A row as the hook talks about it: where it sits and what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowMark {
    /// Index in the *displayed* order — the same number a `table` payload
    /// reports as `selected_index`.
    pub index: usize,
    /// The row's node id. This is what "the same row" means here.
    pub id: String,
}

/// A spawned hook script nobody is waiting for.
pub struct RowChangeRun {
    key: PaneKey,
    /// Script file name, for the failure message.
    name: String,
    child: std::process::Child,
    /// The payload handed over, deleted once the child is gone.
    json_path: std::path::PathBuf,
    /// Where the child's stdout and stderr went. A detached child must not
    /// inherit the terminal the TUI draws on, and a pipe nobody reads fills
    /// up and blocks the script — so it writes to a file, which is also what
    /// the failure notice quotes from.
    log_path: std::path::PathBuf,
}

impl App {
    /// The settle delay, from `script.row_change_delay_ms`.
    fn row_change_delay(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.config.script.row_change_delay_ms)
    }

    /// Look at the focused pane and arm the settle timer when its selected
    /// row is not the one this pane last reported. Cheap on purpose — two
    /// lookups and a string compare, no database, no config read beyond a
    /// number — because it runs on every turn of the main loop.
    ///
    /// Whether any script is *bound* is asked only when the timer fires: a
    /// binding lives in the database, and querying it per cursor move would
    /// put a round trip in the way of `j`.
    pub fn note_row_cursor(&mut self) {
        let Tab::Content(view_index) = self.active_tab;
        let Some(cv) = self.content_view(view_index) else {
            return;
        };
        let key = (view_index, cv.active_pane_id());
        let Some((_, _, id)) = cv.focused_row_mark() else {
            // Nothing selected. Forget the pane's last row so that arriving
            // on one again counts as a change — an emptied list that fills
            // up must not stay silent because the cursor happens to land on
            // the row it left.
            self.row_change_seen.remove(&key);
            return;
        };
        let changed = self
            .row_change_seen
            .get(&key)
            .map_or(true, |seen| seen.id != id);
        if changed {
            self.row_change_deadline = Some(Instant::now() + self.row_change_delay());
        }
    }

    /// When the settle timer is due, for the main loop's `select!`. `None`
    /// parks the branch on a never-resolving future, so an unarmed hook
    /// costs no wakeups.
    pub fn row_change_deadline(&self) -> Option<Instant> {
        self.row_change_deadline
    }

    /// The timer ran out: report the row the cursor came to rest on to every
    /// script bound to [`ScriptHook::RowChange`] on this pane's level.
    ///
    /// Returns whether the screen needs repainting — it never does, the
    /// scripts run outside; the `bool` keeps the call shaped like the other
    /// main-loop branches.
    pub fn fire_row_change_hooks(&mut self) -> bool {
        self.row_change_deadline = None;
        let Tab::Content(view_index) = self.active_tab;
        let Some((key, mark, scope)) = self.content_view(view_index).and_then(|cv| {
            let pane_id = cv.active_pane_id();
            let (_, index, id) = cv.focused_row_mark()?;
            let scope = cv.pane_script_scope(pane_id)?;
            Some(((view_index, pane_id), RowMark { index, id }, scope))
        }) else {
            return false;
        };
        // Moved away and back again while the timer ran: the row on screen is
        // the one already reported, and there is nothing to tell anybody.
        if self.row_change_seen.get(&key) == Some(&mark) {
            return false;
        }
        // A run of this pane's script is still going. Wait it out rather than
        // start a second one, and look again a delay later.
        if self.row_change_runs.iter().any(|run| run.key == key) {
            self.row_change_deadline = Some(Instant::now() + self.row_change_delay());
            return false;
        }
        let mut names: Vec<String> = self
            .hooks_for_scope(&scope)
            .into_iter()
            .filter(|(_, hook)| ScriptHook::parse(hook) == Some(ScriptHook::RowChange))
            .map(|(name, _)| name)
            .collect();
        // Stable order: the menu lists scripts by name, so hooks run in the
        // order the user sees them.
        names.sort();

        let previous = self.row_change_seen.insert(key, mark.clone());
        for name in names {
            self.spawn_row_change_script(key, &name, previous.as_ref(), &mark);
        }
        false
    }

    /// One detached run. Builds the payload the script's own `# scope:`
    /// header asks for — exactly as the menu path would — and adds the
    /// `row_change` block to it.
    fn spawn_row_change_script(
        &mut self,
        key: PaneKey,
        name: &str,
        previous: Option<&RowMark>,
        next: &RowMark,
    ) {
        use crate::config::view_config::ScriptScope;
        let (view_index, pane_id) = key;
        let Some((scope, default_field)) = self
            .content_view(view_index)
            .and_then(|cv| cv.pane_script_action(pane_id))
        else {
            return;
        };
        let ctx = match scope {
            ScriptScope::Node => self.build_content_node_ctx(view_index, pane_id),
            ScriptScope::FilteredSet => self.build_content_batch_ctx(view_index, pane_id),
            ScriptScope::Table => self.build_content_table_ctx(view_index, pane_id, default_field),
        };
        let Some(ctx) = ctx else {
            return;
        };
        let path = ctx.scripts_dir().join(name);
        // A hook whose script was deleted behind the app's back is not an
        // error the user needs on every cursor move — the menu shows the
        // binding, and re-binding clears it.
        if !path.exists() {
            return;
        }
        if let Some(reason) = hook_binding_rejection(ScriptHook::RowChange, &path.to_string_lossy())
        {
            self.notify_row_change_error(format!("Hook script '{name}' not run: {reason}"));
            return;
        }
        let ctx = self.apply_script_scope_header(ctx, &path);
        let payload = crate::app::script::splice_top_level(
            ctx.build_json(),
            "row_change",
            &row_change_block(previous, next),
        );
        match self.spawn_script_detached(&ctx, &path, &payload, ScriptHook::RowChange.as_str()) {
            Ok((child, json_path, log_path)) => self.row_change_runs.push(RowChangeRun {
                key,
                name: name.to_string(),
                child,
                json_path,
                log_path,
            }),
            Err(e) => self.notify_row_change_error(format!("Hook script '{name}': {e}")),
        }
    }

    /// Reap finished runs. Called from the periodic ticker, so a script that
    /// takes a second does not leave a zombie behind and its failure is
    /// still reported.
    ///
    /// A run that finishes while the cursor has moved on re-arms the timer
    /// through [`App::note_row_cursor`]: that is how the "one run at a time"
    /// rule still converges on the row the user is actually looking at.
    pub fn poll_row_change_runs(&mut self) -> bool {
        if self.row_change_runs.is_empty() {
            return false;
        }
        // `(run, status)` pairs — the status is read where the child is
        // taken off the list, so nothing has to be waited for twice.
        let mut finished: Vec<(RowChangeRun, Option<std::process::ExitStatus>)> = Vec::new();
        let mut i = 0;
        while i < self.row_change_runs.len() {
            match self.row_change_runs[i].child.try_wait() {
                Ok(None) => i += 1,
                Ok(Some(status)) => finished.push((self.row_change_runs.remove(i), Some(status))),
                // A child that cannot be waited for is not one we can keep
                // asking about either — take it off the list.
                Err(_) => finished.push((self.row_change_runs.remove(i), None)),
            }
        }
        if finished.is_empty() {
            return false;
        }
        for (run, status) in finished {
            let log = std::fs::read_to_string(&run.log_path).unwrap_or_default();
            let _ = std::fs::remove_file(&run.json_path);
            let _ = std::fs::remove_file(&run.log_path);
            match status {
                Some(status) if !status.success() => {
                    let tail = log.trim().lines().last().unwrap_or("").trim().to_string();
                    let name = run.name;
                    self.notify_row_change_error(if tail.is_empty() {
                        format!("Row-change hook '{name}' exited with {status}")
                    } else {
                        format!("Row-change hook '{name}': {tail}")
                    });
                }
                // A clean run clears the memory of the last failure, so the
                // same message notifies again if the script breaks anew.
                _ => self.row_change_last_error = None,
            }
        }
        self.note_row_cursor();
        false
    }

    /// Report a hook failure — but only once while it keeps saying the same
    /// thing. The hook fires per cursor move, so a script that is simply
    /// broken would otherwise write one line into the notification log for
    /// every `j` the user presses.
    fn notify_row_change_error(&mut self, message: String) {
        if self.row_change_last_error.as_deref() == Some(message.as_str()) {
            return;
        }
        self.row_change_last_error = Some(message.clone());
        self.notify_error(message);
    }
}

/// The `row_change` block of the payload. `previous_*` is `null` on the
/// first row a pane reports — a level that just opened, a tab entered for
/// the first time — which is how a script tells "the cursor arrived" from
/// "the cursor moved".
fn row_change_block(previous: Option<&RowMark>, next: &RowMark) -> String {
    let (prev_index, prev_id) = match previous {
        Some(mark) => (
            mark.index.to_string(),
            crate::app::script::json_string(&mark.id),
        ),
        None => ("null".to_string(), "null".to_string()),
    };
    format!(
        "{{\"previous_index\": {prev_index}, \"previous_id\": {prev_id}, \
         \"next_index\": {next_index}, \"next_id\": {next_id}}}",
        next_index = next.index,
        next_id = crate::app::script::json_string(&next.id),
    )
}

/// Where the per-pane marks live on the App. A `HashMap` and not one field:
/// the last reported row is remembered *per pane*, so switching tabs and
/// coming back to an unmoved cursor is not a row change.
pub type RowMarks = HashMap<PaneKey, RowMark>;

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(index: usize, id: &str) -> RowMark {
        RowMark {
            index,
            id: id.to_string(),
        }
    }

    #[test]
    fn a_move_names_both_rows() {
        let block = row_change_block(Some(&mark(3, "a")), &mark(4, "b"));
        assert_eq!(
            block,
            "{\"previous_index\": 3, \"previous_id\": \"a\", \"next_index\": 4, \"next_id\": \"b\"}"
        );
    }

    /// Arriving is a row change too, and the script has to be able to see
    /// that it is one.
    #[test]
    fn an_arrival_has_no_previous_row() {
        let block = row_change_block(None, &mark(0, "a"));
        assert_eq!(
            block,
            "{\"previous_index\": null, \"previous_id\": null, \"next_index\": 0, \"next_id\": \"a\"}"
        );
    }

    /// Node ids come from adapters, not from a vocabulary we control.
    #[test]
    fn an_id_with_a_quote_in_it_stays_valid_json() {
        let block = row_change_block(None, &mark(1, "a\"b"));
        assert!(block.contains("\"next_id\": \"a\\\"b\""), "{block}");
        let parsed: serde_json::Value = serde_json::from_str(&block).expect("valid JSON");
        assert_eq!(parsed["next_id"], "a\"b");
    }
}
