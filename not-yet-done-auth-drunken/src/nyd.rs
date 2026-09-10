//! This side of the boundary: the line protocol nyd speaks to a plugin.
//!
//! The shapes come from `not-yet-done-content`, so the plugin and the
//! runtime cannot drift apart on what a line looks like. What is written
//! here is only the *reading and writing* of them, which a plugin in any
//! other language would write for itself.

use not_yet_done_content::auth::{PluginLine, ToPlugin};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

/// nyd's end of the pipes.
pub struct Nyd {
    lines: mpsc::Receiver<Result<ToPlugin, String>>,
}

impl Nyd {
    /// Start reading stdin.
    ///
    /// On a task, and not read where it is needed, for the reason the
    /// browser's socket is: the main loop waits on this and on the browser
    /// at once, and a `select!` that cancels a half-read line loses it.
    pub fn on_stdin() -> Self {
        let (tx, lines) = mpsc::channel(8);
        tokio::spawn(async move {
            let mut reader = BufReader::new(tokio::io::stdin()).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                let read = serde_json::from_str::<ToPlugin>(&line)
                    .map_err(|e| format!("nyd sent a line this plugin cannot read: {e}"));
                if tx.send(read).await.is_err() {
                    return;
                }
            }
        });
        Self { lines }
    }

    /// The next thing nyd says, or `None` once it has closed our stdin —
    /// which is the second half of "give up", said without words.
    pub async fn next(&mut self) -> Option<Result<ToPlugin, String>> {
        self.lines.recv().await
    }
}

/// Say one line to nyd.
///
/// Straight to a locked stdout and flushed, because a plugin's output is a
/// conversation: a `form` still sitting in a buffer is a dialog that never
/// opens, and the deadlock that follows looks like a plugin that hung.
pub fn say(line: PluginLine) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    match serde_json::to_string(&line) {
        Ok(text) => {
            let _ = writeln!(out, "{text}");
            let _ = out.flush();
        }
        // Nothing in a `PluginLine` can fail to serialize; if one somehow
        // does, the log is the only place left to say so.
        Err(e) => eprintln!("nyd-auth-drunken: could not write a line: {e}"),
    }
}
