//! Connection supervision for the adapter CLI — the terminal counterpart to
//! the TUI's auth banner and credential popup.
//!
//! The TUI watches [`AdapterStatus`] for the whole session: it renders
//! "Connecting… (1/3) Timeout: 30s", opens a form when the adapter asks for
//! credentials, and shows the data once the adapter reports `Ready`. The CLI
//! needs exactly the same three things, minus the keypress — a command
//! connects immediately instead of waiting for `r`:
//!
//! - progress and failures go to **stderr**, so stdout stays pipeable,
//! - [`AdapterStatus::NeedsCreds`] is answered by asking on the terminal,
//! - a command that would list against a not-yet-connected adapter waits for
//!   the connection first.
//!
//! That last point is what makes an empty list trustworthy. Adapters whose
//! rows come from a live connection (Stoat) build that connection in the
//! background and project an empty snapshot until it stands; without the wait
//! the CLI would print nothing and exit 0, which reads as "the account has no
//! content" rather than "not connected yet". The contract that makes the wait
//! possible: **an adapter that connects in the background must publish
//! `Connecting` synchronously**, before the work is spawned — otherwise
//! `Idle` is ambiguous between "about to connect" and "never will".
//!
//! Without a terminal (a script, a cron job, a pipe) asking is impossible, and
//! the command fails with a message naming the fields and the non-interactive
//! providers that can supply them — never with a silent empty result.

use std::collections::HashMap;
use std::io::IsTerminal;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use not_yet_done_content::{AdapterStatus, AuthField, ContentAdapter};
use tokio::sync::oneshot;

/// How long a command waits for an adapter that says it is connecting before
/// giving up. Generous — a cold login can involve a cookie script, an SSO
/// bounce and a WebSocket handshake — but finite, so a hung backend fails the
/// command instead of parking the terminal forever.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);

/// Watches the adapter's status for the duration of one CLI command.
///
/// Owns the credential prompt: whenever the adapter asks for credentials the
/// supervisor collects them on the terminal and submits them, which unblocks
/// whichever adapter call is waiting for the login. Conditions it cannot
/// resolve (no terminal to ask on, an adapter that asks but cannot accept a
/// submission) travel back through `fatal` and abort the command — the
/// alternative would be waiting on a login that can never complete.
pub struct Supervisor {
    task: tokio::task::JoinHandle<()>,
    fatal: Option<oneshot::Receiver<String>>,
}

/// Where credential values come from. Behind a trait so the supervisor's
/// behaviour around a prompt is testable without a terminal — the real
/// implementation is the only one that talks to one.
#[async_trait::async_trait]
pub trait Prompter: Send + Sync {
    async fn ask(&self, fields: Vec<AuthField>) -> Result<HashMap<String, String>>;
}

/// Asks on the terminal; fails with a usable explanation when there is none.
pub struct TerminalPrompter;

#[async_trait::async_trait]
impl Prompter for TerminalPrompter {
    async fn ask(&self, fields: Vec<AuthField>) -> Result<HashMap<String, String>> {
        prompt_credentials(fields).await
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Supervisor {
    pub fn start(adapter: Arc<dyn ContentAdapter>) -> Self {
        Self::start_with(adapter, Arc::new(TerminalPrompter))
    }

    pub fn start_with(adapter: Arc<dyn ContentAdapter>, prompter: Arc<dyn Prompter>) -> Self {
        let (fatal_tx, fatal_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            if let Err(e) = watch_status(adapter, prompter).await {
                let _ = fatal_tx.send(e.to_string());
            }
        });
        Self {
            task,
            fatal: Some(fatal_rx),
        }
    }

    /// Run `fut`, cutting it short if the supervisor hits a condition that
    /// makes finishing impossible. A future blocked inside the adapter's auth
    /// path cannot be cancelled from the outside, so the race is the only way
    /// out of "waiting for a prompt nobody can answer".
    pub async fn guard<T, F>(&mut self, fut: F) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>>,
    {
        let Some(fatal) = self.fatal.as_mut() else {
            return fut.await;
        };
        tokio::select! {
            res = fut => res,
            recv = fatal => {
                // A closed channel just means the supervisor ended without a
                // verdict; only an actual message is fatal.
                match recv {
                    Ok(msg) => Err(anyhow!(msg)),
                    Err(_) => {
                        self.fatal = None;
                        Err(anyhow!("adapter status supervisor stopped unexpectedly"))
                    }
                }
            }
        }
    }
}

/// What makes one credential form distinguishable from the next, for the
/// benefit of the phantom-prompt guard in [`watch_status`].
#[derive(PartialEq, Eq)]
struct PromptKey {
    names: Vec<String>,
    header: Option<String>,
    error: Option<String>,
}

/// Report status changes on stderr and answer credential requests. Returns
/// only on an unanswerable request; a healthy adapter keeps it parked.
async fn watch_status(adapter: Arc<dyn ContentAdapter>, prompter: Arc<dyn Prompter>) -> Result<()> {
    let mut rx = adapter.subscribe_status();
    let mut last_line: Option<String> = None;
    let mut last_prompted: Option<PromptKey> = None;
    loop {
        let status = rx.borrow_and_update().clone();
        match &status {
            AdapterStatus::NeedsCreds {
                fields,
                header,
                error,
            } => {
                // A watch channel can hand out the same state twice (a fresh
                // borrow after an unrelated wakeup); asking for the identical
                // form again would be a phantom second prompt. The header and
                // the error are part of the key because a credential script
                // re-asks the *same* field after a wrong answer, and that is a
                // genuine second prompt.
                let key = PromptKey {
                    names: fields.iter().map(|f| f.name.clone()).collect(),
                    header: header.clone(),
                    error: error.clone(),
                };
                if last_prompted.as_ref() != Some(&key) {
                    last_prompted = Some(key);
                    if let Some(h) = header {
                        eprintln!("nyd: {h}");
                    }
                    if let Some(e) = error {
                        eprintln!("nyd: {e}");
                    }
                    let values = match prompter.ask(fields.clone()).await {
                        Ok(v) => v,
                        Err(e) => {
                            // Nobody will answer this form. Say so, or the
                            // adapter waits out its login on a prompt that
                            // was never shown.
                            let _ = adapter.cancel_credentials().await;
                            return Err(e);
                        }
                    };
                    adapter
                        .submit_credentials(values)
                        .await
                        .map_err(|e| anyhow!("submitting credentials failed: {e}"))?;
                }
            }
            other => {
                last_prompted = None;
                if let Some(line) = other.banner_text() {
                    // Only on change — a Busy countdown ticking once a second
                    // must not scroll the terminal.
                    if last_line.as_deref() != Some(line.as_str()) {
                        eprintln!("nyd: {line}");
                        last_line = Some(line);
                    }
                }
            }
        }
        if rx.changed().await.is_err() {
            return Ok(());
        }
    }
}

/// Wait until the adapter has finished connecting.
///
/// `Idle` returns immediately: it means no connection attempt is running, so
/// there is nothing to wait for (adapters that never connect asynchronously
/// report `Ready` from the start; one that does announces `Connecting` before
/// spawning the attempt). `Busy` also returns — it implies a live connection
/// with a request in flight, which the call itself will wait out.
pub async fn wait_until_connected(adapter: &dyn ContentAdapter) -> Result<()> {
    let mut rx = adapter.subscribe_status();
    let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
    loop {
        let status = rx.borrow_and_update().clone();
        match &status {
            AdapterStatus::Ready | AdapterStatus::Idle | AdapterStatus::Busy { .. } => {
                return Ok(());
            }
            AdapterStatus::Failed { reason } => bail!("connection failed: {reason}"),
            AdapterStatus::Connecting { .. } | AdapterStatus::NeedsCreds { .. } => {}
        }
        match tokio::time::timeout_at(deadline, rx.changed()).await {
            Ok(Ok(())) => {}
            // Sender dropped: nobody will ever report `Ready`, so stop waiting
            // and let the command run against whatever the adapter has.
            Ok(Err(_)) => return Ok(()),
            Err(_) => bail!(
                "timed out after {}s waiting for the connection (last status: {})",
                CONNECT_TIMEOUT.as_secs(),
                status.banner_text().unwrap_or_else(|| "unknown".into())
            ),
        }
    }
}

/// Ask for the credential fields on the terminal.
///
/// Reads via dialoguer (which prompts on stderr, keeping stdout clean) on a
/// blocking thread. Without a TTY there is nothing to ask on, and the error
/// says which fields are missing and how to supply them non-interactively —
/// the case that used to end in an empty list and exit 0.
async fn prompt_credentials(fields: Vec<AuthField>) -> Result<HashMap<String, String>> {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        let names = fields
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "this connection needs credentials ({names}) but there is no terminal to ask on. \
             Give those fields a non-interactive credential provider (env, file, command or \
             keyring) in the connection config, or run the command in a terminal."
        );
    }
    tokio::task::spawn_blocking(move || {
        let mut out = HashMap::new();
        for f in &fields {
            let value = if f.masked {
                dialoguer::Password::new()
                    .with_prompt(&f.label)
                    .allow_empty_password(f.optional)
                    .interact()
                    .map_err(|e| anyhow!("reading {}: {e}", f.name))?
            } else {
                let mut input = dialoguer::Input::<String>::new().with_prompt(&f.label);
                if let Some(prefill) = &f.prefill {
                    input = input.with_initial_text(prefill);
                }
                if f.optional {
                    input = input.allow_empty(true);
                }
                input
                    .interact_text()
                    .map_err(|e| anyhow!("reading {}: {e}", f.name))?
            };
            out.insert(f.name.clone(), value);
        }
        Ok(out)
    })
    .await
    .map_err(|e| anyhow!("credential prompt task failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::{ContentAdapter, ContentError};
    use tokio::sync::watch;

    /// Minimal adapter whose status we drive by hand.
    struct StatusOnly {
        rx: watch::Receiver<AdapterStatus>,
    }

    #[async_trait::async_trait]
    impl ContentAdapter for StatusOnly {
        fn adapter_type(&self) -> &str {
            "test"
        }
        fn instance_id(&self) -> &str {
            "test"
        }
        fn childs<'a>(
            &'a self,
            _node: &'a dyn not_yet_done_content::Node,
        ) -> Vec<not_yet_done_content::children::Child<'a>> {
            Vec::new()
        }
        async fn root(&self) -> not_yet_done_content::Result<Box<dyn not_yet_done_content::Node>> {
            Err(ContentError::NotSupported("no root".into()))
        }
        async fn get_by_id(
            &self,
            _id: &str,
        ) -> not_yet_done_content::Result<Box<dyn not_yet_done_content::Node>> {
            Err(ContentError::NotSupported("no nodes".into()))
        }
        fn subscribe_status(&self) -> watch::Receiver<AdapterStatus> {
            self.rx.clone()
        }
    }

    #[tokio::test]
    async fn ready_and_idle_do_not_wait() {
        for start in [AdapterStatus::Ready, AdapterStatus::Idle] {
            let (_tx, rx) = watch::channel(start);
            let a = StatusOnly { rx };
            wait_until_connected(&a).await.expect("returns at once");
        }
    }

    #[tokio::test]
    async fn waits_for_connecting_to_become_ready() {
        let (tx, rx) = watch::channel(AdapterStatus::Connecting {
            retry: 1,
            max_retries: 1,
            timeout_secs: 0,
        });
        let a = StatusOnly { rx };
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let _ = tx.send(AdapterStatus::Ready);
        });
        wait_until_connected(&a).await.expect("becomes ready");
    }

    #[tokio::test]
    async fn failure_while_connecting_is_reported() {
        let (tx, rx) = watch::channel(AdapterStatus::Connecting {
            retry: 1,
            max_retries: 1,
            timeout_secs: 0,
        });
        let a = StatusOnly { rx };
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let _ = tx.send(AdapterStatus::Failed {
                reason: "no route to host".into(),
            });
        });
        let err = wait_until_connected(&a).await.expect_err("fails");
        assert!(err.to_string().contains("no route to host"));
    }

    /// The case this module exists for: a prompt that cannot be answered ends
    /// the command with an explanation instead of leaving it hanging (or, one
    /// layer up, printing an empty list).
    #[tokio::test]
    async fn unanswerable_prompt_aborts_the_guarded_future() {
        let (_tx, rx) = watch::channel(AdapterStatus::NeedsCreds {
            fields: vec![AuthField {
                name: "password".into(),
                label: "Password".into(),
                masked: true,
                optional: false,
                prefill: None,
            }],
            header: None,
            error: None,
        });
        let adapter: Arc<dyn ContentAdapter> = Arc::new(StatusOnly { rx });
        // Stands in for "no terminal to ask on" without depending on how the
        // test binary's streams happen to be wired.
        struct NoTerminal;
        #[async_trait::async_trait]
        impl Prompter for NoTerminal {
            async fn ask(&self, fields: Vec<AuthField>) -> Result<HashMap<String, String>> {
                bail!(
                    "no terminal to ask on ({})",
                    fields
                        .iter()
                        .map(|f| f.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        let mut sup = Supervisor::start_with(adapter, Arc::new(NoTerminal));
        // Stands in for an adapter call parked on the login that will never
        // complete.
        let err = sup
            .guard(async {
                std::future::pending::<()>().await;
                Ok(())
            })
            .await
            .expect_err("aborted");
        let msg = err.to_string();
        assert!(msg.contains("password"), "names the field: {msg}");
    }
}
