//! Running a step's command (Phase 4 `auto`) or the AI runner (Phase 5 `ai`).
//!
//! Both run through `sh -c` so the definition (or the configured `ai_command`)
//! can use pipes, redirection, and `&&` exactly as written; stdout and stderr
//! are captured together into the step's protocol output, and the process exit
//! status decides the step's terminal state (`done` on success, `failed`
//! otherwise). No routing or variable extraction happens here — that is Phase 6;
//! this module only *carries out* one step and reports the outcome.
//!
//! The `ai` runner is deliberately generic: it is any command the user
//! configures (`workflow.ai_command`), handed the step's instruction as the
//! prompt over **stdin** plus a set of `NYD_WORKFLOW_*` environment variables —
//! exactly like `reminder.command`, with no binding to any particular AI tool.
//! The command is expected to drive the app's own CLI to carry the step out.

use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::store::step_status;

/// The outcome of running one step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepResult {
    /// Terminal step status: [`step_status::DONE`] or [`step_status::FAILED`].
    pub status: &'static str,
    /// Captured stdout followed by stderr (and, on failure, an exit-code note).
    /// This is the human-facing text kept in the run protocol.
    pub output: String,
    /// The process exit code, or `None` when it was terminated by a signal or
    /// never spawned. Used by route guards (Phase 6b2).
    pub exit: Option<i32>,
    /// Captured standard output, kept separate for route guards.
    pub stdout: String,
    /// Captured standard error, kept separate for route guards.
    pub stderr: String,
}

/// The run context handed to the AI runner as environment variables (and, for
/// the prompt, stdin).
pub struct AiContext<'a> {
    pub run_id: &'a str,
    pub step_id: &'a str,
    pub title: &'a str,
    /// The step's instruction — the prompt fed to the AI over stdin.
    pub prompt: &'a str,
}

/// Run `command` through `sh -c`, capturing stdout+stderr and mapping the exit
/// status to a terminal step status. A spawn failure (no `sh`, etc.) is itself a
/// `failed` result carrying the error text, so a broken command never propagates
/// as an adapter error — it is recorded as a failed step like any other.
pub async fn run_command(command: &str) -> StepResult {
    match Command::new("sh").arg("-c").arg(command).output().await {
        Ok(out) => outcome(out),
        Err(e) => spawn_failed(e),
    }
}

/// Run the configured `ai_command` through `sh -c`, feeding `cx.prompt` on stdin
/// and the run context as `NYD_WORKFLOW_*` env vars, then map the exit status to
/// a terminal step status (as [`run_command`] does). The prompt is small (a step
/// instruction), so writing it before draining stdout does not risk a pipe
/// deadlock.
pub async fn run_ai(ai_command: &str, cx: AiContext<'_>) -> StepResult {
    let spawned = Command::new("sh")
        .arg("-c")
        .arg(ai_command)
        .env("NYD_WORKFLOW_RUN_ID", cx.run_id)
        .env("NYD_WORKFLOW_STEP_ID", cx.step_id)
        .env("NYD_WORKFLOW_STEP_TITLE", cx.title)
        .env("NYD_WORKFLOW_PROMPT", cx.prompt)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => return spawn_failed(e),
    };
    if let Some(mut stdin) = child.stdin.take() {
        // Best-effort: a command that ignores stdin closes its read end, which
        // surfaces here as a write error we intentionally discard.
        let _ = stdin.write_all(cx.prompt.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }
    match child.wait_with_output().await {
        Ok(out) => outcome(out),
        Err(e) => spawn_failed(e),
    }
}

/// Map a finished process's captured streams and exit status to a [`StepResult`].
fn outcome(out: std::process::Output) -> StepResult {
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let exit = out.status.code();

    // The protocol output merges stdout and stderr (and, on failure, an
    // exit-code note); the separate streams above feed route guards.
    let mut output = stdout.clone();
    if !stderr.trim().is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&stderr);
    }
    if out.status.success() {
        return StepResult {
            status: step_status::DONE,
            output,
            exit,
            stdout,
            stderr,
        };
    }
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    match exit {
        Some(code) => output.push_str(&format!("[exit {code}]")),
        None => output.push_str("[terminated by signal]"),
    }
    StepResult {
        status: step_status::FAILED,
        output,
        exit,
        stdout,
        stderr,
    }
}

/// A spawn failure recorded as a failed step rather than an adapter error.
fn spawn_failed(e: std::io::Error) -> StepResult {
    let msg = format!("failed to spawn command: {e}");
    StepResult {
        status: step_status::FAILED,
        output: msg.clone(),
        exit: None,
        stdout: String::new(),
        stderr: msg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn success_captures_stdout() {
        let r = run_command("echo hello").await;
        assert_eq!(r.status, step_status::DONE);
        assert!(r.output.contains("hello"));
    }

    #[tokio::test]
    async fn nonzero_exit_is_failed_with_code() {
        let r = run_command("echo oops >&2; exit 3").await;
        assert_eq!(r.status, step_status::FAILED);
        assert!(r.output.contains("oops"));
        assert!(r.output.contains("[exit 3]"));
    }

    #[tokio::test]
    async fn separate_streams_and_exit_code_are_captured() {
        let r = run_command("echo out; echo err >&2; exit 5").await;
        assert_eq!(r.exit, Some(5));
        assert!(r.stdout.contains("out"));
        assert!(!r.stdout.contains("err"));
        assert!(r.stderr.contains("err"));
        assert!(!r.stderr.contains("out"));
    }

    #[tokio::test]
    async fn ai_runner_receives_prompt_on_stdin_and_env() {
        // Echo back the prompt (stdin) and the step id (env) so we can assert
        // both channels reached the command.
        let r = run_ai(
            "cat; printf ' id=%s' \"$NYD_WORKFLOW_STEP_ID\"",
            AiContext {
                run_id: "r1",
                step_id: "greet",
                title: "Greet",
                prompt: "do the thing",
            },
        )
        .await;
        assert_eq!(r.status, step_status::DONE);
        assert!(r.output.contains("do the thing"));
        assert!(r.output.contains("id=greet"));
    }
}
