//! One connection actor per account.
//!
//! IMAP has no request multiplexing: a session has *one* selected mailbox and
//! runs *one* command at a time. Two concurrent callers on one session do not
//! interleave — they corrupt each other's responses. The session therefore
//! lives inside a task, and everything reaches it as a message on a channel.
//! That the tree, the list and a background reload all ask at once is then a
//! queue, not a race.
//!
//! The actor also owns the *lifecycle*: it connects lazily (opening the tab
//! for one account must not log into the other five), and it reconnects once
//! when a request dies on a session the server has timed out — an hour of
//! idling followed by a folder switch is the normal case, not an error worth
//! showing.
//!
//! Opening the connection is itself repeated, per the account's `retry:`. A
//! refused socket is the failure that most often goes away on its own — a
//! server restarting, a VPN route that came up a moment after the tab did —
//! and showing it as a failed account makes the user clear an error that had
//! already passed. Three questions, deliberately kept apart: whether the
//! session survives ([`MailError::is_fatal`]), whether the *command* is worth
//! running again on a new one ([`MailError::is_worth_retrying`]), and whether
//! *opening* the connection is worth another go
//! ([`MailError::is_worth_reconnecting`]).
//!
//! Every command runs under a deadline. Not because a slow server is an
//! error, but because a connection that neither answers nor closes would
//! otherwise be waited on forever — and with one command at a time, that one
//! request takes the account's whole queue with it. The wait the user sees is
//! then bounded by what the config says, not by the TCP stack.
//!
//! Adding a command means: a function in [`super::ops`], a variant in
//! [`Request`], one arm in [`Actor::serve`], and a method on [`Connection`].

use std::sync::Arc;
use std::time::Duration;

use not_yet_done_content::{RetryConfig, StatusReporter};
use tokio::sync::{mpsc, oneshot};

use super::login::{self, Attempt, MailSession};
use super::ops::{self, MessagePage};
use super::session::SessionState;
use crate::config::AccountConfig;
use crate::credentials::AccountCredentials;
use crate::error::{MailError, MailResult};
use crate::model::{EnvelopeRow, FolderInfo};

/// How many requests may queue up before the caller waits. Deep enough that
/// a burst of view loads never blocks, shallow enough to notice a stall.
const QUEUE_DEPTH: usize = 32;

/// What the actor can be asked to do.
pub(crate) enum Request {
    /// Make sure the account is logged in, and report what the server can do.
    Capabilities {
        reply: oneshot::Sender<MailResult<Vec<String>>>,
    },
    /// Every mailbox the account has, minus the ones its config excludes.
    Folders {
        reply: oneshot::Sender<MailResult<Vec<FolderInfo>>>,
    },
    /// Message and unread counts for one mailbox.
    FolderStatus {
        path: String,
        reply: oneshot::Sender<MailResult<(u32, u32)>>,
    },
    /// One page of a folder's messages.
    Messages {
        path: String,
        query: String,
        offset: u32,
        limit: u32,
        reply: oneshot::Sender<MailResult<MessagePage>>,
    },
    /// The whole source of one message, for reading it.
    Body {
        path: String,
        uid_validity: u32,
        uid: u32,
        reply: oneshot::Sender<MailResult<Vec<u8>>>,
    },
    /// The decoded bytes of one MIME part, for saving or opening a file.
    Part {
        path: String,
        uid_validity: u32,
        uid: u32,
        part: String,
        reply: oneshot::Sender<MailResult<Vec<u8>>>,
    },
    /// The envelope of a single message, for a row nobody listed.
    Envelope {
        path: String,
        uid_validity: u32,
        uid: u32,
        reply: oneshot::Sender<MailResult<EnvelopeRow>>,
    },
    /// Put a message we just sent into the account's Sent mailbox.
    ///
    /// Which mailbox that is, is decided *here* rather than by the caller:
    /// the actor holds the account config, so the `sent_folder:` override and
    /// the server's own `\Sent` are both within reach, and the reply names
    /// the folder the copy landed in.
    AppendToSent {
        source: Vec<u8>,
        reply: oneshot::Sender<MailResult<String>>,
    },
    /// Add IMAP flags to one message.
    AddFlags {
        path: String,
        uid_validity: u32,
        uid: u32,
        flags: String,
        reply: oneshot::Sender<MailResult<()>>,
    },
    /// Log out and drop the session. The actor stays alive: the next request
    /// connects again.
    Disconnect {
        reply: oneshot::Sender<MailResult<()>>,
    },
}

/// A handle on one account's connection. Cheap to clone; dropping the last
/// one ends the actor, which logs out on its way down.
#[derive(Clone)]
pub(crate) struct Connection {
    tx: mpsc::Sender<Request>,
}

impl Connection {
    /// Start the actor. Nothing is connected yet — the first request is.
    pub(crate) fn spawn(
        account: Arc<AccountConfig>,
        creds: Arc<AccountCredentials>,
        status: StatusReporter,
        command_timeout: Duration,
        retry: RetryConfig,
    ) -> Self {
        let (tx, rx) = mpsc::channel(QUEUE_DEPTH);
        let actor = Actor {
            account,
            creds,
            status,
            command_timeout,
            retry,
            session: None,
        };
        tokio::spawn(actor.run(rx));
        Self { tx }
    }

    /// Connect if necessary and hand back the server's capability list.
    pub(crate) async fn capabilities(&self) -> MailResult<Vec<String>> {
        self.ask(|reply| Request::Capabilities { reply }).await
    }

    /// The account's folder tree, already ordered and filtered.
    pub(crate) async fn folders(&self) -> MailResult<Vec<FolderInfo>> {
        self.ask(|reply| Request::Folders { reply }).await
    }

    /// `(total, unread)` for one mailbox. Separate from [`Connection::folders`]
    /// because `STATUS` costs a round trip *per folder* — some servers open
    /// the mailbox internally to answer it — so the counts are fetched for the
    /// folders that are actually on screen, not for all two hundred.
    pub(crate) async fn folder_status(&self, path: &str) -> MailResult<(u32, u32)> {
        let path = path.to_string();
        self.ask(|reply| Request::FolderStatus { path, reply })
            .await
    }

    /// One page of a folder, matching an IMAP SEARCH query. The window is
    /// what reaches the wire as a `UID FETCH`: a folder of a hundred thousand
    /// messages costs one search and one page of envelopes.
    pub(crate) async fn messages(
        &self,
        path: &str,
        query: &str,
        offset: u32,
        limit: u32,
    ) -> MailResult<MessagePage> {
        let (path, query) = (path.to_string(), query.to_string());
        self.ask(|reply| Request::Messages {
            path,
            query,
            offset,
            limit,
            reply,
        })
        .await
    }

    /// One message's source, whole and unread-preserving. What the preview
    /// pane renders from.
    pub(crate) async fn body(
        &self,
        path: &str,
        uid_validity: u32,
        uid: u32,
    ) -> MailResult<Vec<u8>> {
        let path = path.to_string();
        self.ask(|reply| Request::Body {
            path,
            uid_validity,
            uid,
            reply,
        })
        .await
    }

    /// The decoded bytes of one MIME part — an attachment as a file.
    pub(crate) async fn part(
        &self,
        path: &str,
        uid_validity: u32,
        uid: u32,
        part: &str,
    ) -> MailResult<Vec<u8>> {
        let (path, part) = (path.to_string(), part.to_string());
        self.ask(|reply| Request::Part {
            path,
            uid_validity,
            uid,
            part,
            reply,
        })
        .await
    }

    /// The envelope of one message. Used where a message is reached without
    /// the page that listed it — a restored cursor, a bookmarked attachment
    /// level — and never on the listing path, which gets its rows in bulk.
    pub(crate) async fn envelope(
        &self,
        path: &str,
        uid_validity: u32,
        uid: u32,
    ) -> MailResult<EnvelopeRow> {
        let path = path.to_string();
        self.ask(|reply| Request::Envelope {
            path,
            uid_validity,
            uid,
            reply,
        })
        .await
    }

    /// File a copy of an outgoing message in Sent, and say where it went.
    pub(crate) async fn append_to_sent(&self, source: Vec<u8>) -> MailResult<String> {
        self.ask(|reply| Request::AppendToSent { source, reply })
            .await
    }

    /// Add flags to one message — `\Answered` after a reply went out.
    pub(crate) async fn add_flags(
        &self,
        path: &str,
        uid_validity: u32,
        uid: u32,
        flags: &str,
    ) -> MailResult<()> {
        let (path, flags) = (path.to_string(), flags.to_string());
        self.ask(|reply| Request::AddFlags {
            path,
            uid_validity,
            uid,
            flags,
            reply,
        })
        .await
    }

    /// A handle with no actor behind it. Only for tests that must build a
    /// node without a server — the first request on it fails as `Closed`.
    #[cfg(test)]
    pub(crate) fn from_sender(tx: mpsc::Sender<Request>) -> Self {
        Self { tx }
    }

    /// Drop the session (a manual reconnect, or a tab being put away).
    pub(crate) async fn disconnect(&self) -> MailResult<()> {
        self.ask(|reply| Request::Disconnect { reply }).await
    }

    async fn ask<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<MailResult<T>>) -> Request,
    ) -> MailResult<T> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(build(tx))
            .await
            .map_err(|_| MailError::Closed)?;
        rx.await.map_err(|_| MailError::Closed)?
    }
}

struct Actor {
    account: Arc<AccountConfig>,
    creds: Arc<AccountCredentials>,
    /// This account's own status channel. Labels here already name the
    /// account; the adapter merges the channels and only has to name it in
    /// the credential dialog's header, which the auth layer writes.
    status: StatusReporter,
    /// How long one command may take. Zero means no deadline at all.
    command_timeout: Duration,
    /// How often opening the connection is attempted before the failure is
    /// shown to the user. Applies to the *login*, never to a command: a
    /// command runs on a session that already exists, and its own repeat rule
    /// is the one in [`on_session`].
    retry: RetryConfig,
    session: Option<SessionState>,
}

/// Run one command under the account's deadline.
///
/// The timeout drops the command's future mid-await, which leaves the session
/// with a half-read response on the wire — so the error it returns is fatal
/// and the session goes. What it is *not* is worth another attempt: see
/// [`MailError::is_worth_retrying`].
async fn under_deadline<T>(
    limit: Duration,
    label: &str,
    call: impl std::future::Future<Output = MailResult<T>>,
) -> MailResult<T> {
    if limit.is_zero() {
        return call.await;
    }
    match tokio::time::timeout(limit, call).await {
        Ok(outcome) => outcome,
        Err(_) => Err(MailError::Timeout(format!(
            "{label}: no answer in {}s — giving up on this connection",
            limit.as_secs()
        ))),
    }
}

/// Run one command against the account's session, with the lifecycle around
/// it: connect if there is nothing, announce the request, and — if a *reused*
/// session died under it — reconnect and run it once more.
///
/// A macro rather than a function because the command borrows the session
/// across an await, which a generic callback could only express by boxing
/// every call site's future.
macro_rules! on_session {
    ($self:ident, $label:expr, |$s:ident| $call:expr) => {{
        loop {
            let (mut owned, fresh) = match $self.take_session().await {
                Ok(v) => v,
                Err(e) => break Err(e),
            };
            let label = $label;
            let limit = $self.command_timeout;
            // The announcement names the deadline it runs under, so the
            // banner counts towards something instead of just counting.
            let announced = $self.status.busy(label.clone(), limit.as_secs());
            let outcome = {
                let $s: &mut SessionState = &mut owned;
                under_deadline(limit, &label, $call).await
            };
            drop(announced);
            match outcome {
                Ok(v) => {
                    $self.session = Some(owned);
                    break Ok(v);
                }
                Err(e) if e.is_fatal() => {
                    // `owned` is not put back: the session is gone with it.
                    if e.is_auth() {
                        $self.creds.invalidate().await;
                    }
                    if fresh || !e.is_worth_retrying() {
                        // Either it failed on a session we had just built, so
                        // trying again would only repeat it — or it is the
                        // kind of failure a new session does not cure.
                        $self.status.failed(e.to_string());
                        break Err(e);
                    }
                    continue;
                }
                Err(e) => {
                    // The server refused the command; the session is fine.
                    $self.session = Some(owned);
                    break Err(e);
                }
            }
        }
    }};
}

impl Actor {
    async fn run(mut self, mut rx: mpsc::Receiver<Request>) {
        while let Some(req) = rx.recv().await {
            self.serve(req).await;
        }
        // Every handle is gone: say goodbye rather than letting the server
        // find out by timeout.
        self.close().await;
    }

    async fn serve(&mut self, req: Request) {
        match req {
            Request::Capabilities { reply } => {
                let out = on_session!(self, self.label("Contacting the server"), |s| {
                    ops::capabilities(s)
                });
                let _ = reply.send(out);
            }
            Request::Folders { reply } => {
                let out = on_session!(self, self.label("Listing folders"), |s| {
                    ops::list_folders(s)
                });
                let excluded = &self.account.exclude_folders;
                let out = out.map(|folders| {
                    folders
                        .into_iter()
                        .filter(|f| !f.is_excluded(excluded))
                        .collect()
                });
                let _ = reply.send(out);
            }
            Request::FolderStatus { path, reply } => {
                let out = on_session!(self, self.label(&format!("Reading {path}")), |s| {
                    ops::folder_status(s, &path)
                });
                let _ = reply.send(out);
            }
            Request::Messages {
                path,
                query,
                offset,
                limit,
                reply,
            } => {
                // The label names the window, not just the folder: waiting on
                // rows 5000-5050 of a big mailbox should look different from
                // waiting on the first page. It is rebuilt per attempt because
                // a retry announces itself again.
                let out = on_session!(
                    self,
                    self.label(&format!("Loading {path} {}-{}", offset + 1, offset + limit)),
                    |s| ops::messages(s, &path, &query, offset, limit)
                );
                let _ = reply.send(out);
            }
            Request::Body {
                path,
                uid_validity,
                uid,
                reply,
            } => {
                let out = on_session!(self, self.label(&format!("Reading {path} #{uid}")), |s| {
                    ops::message_source(s, &path, uid_validity, uid)
                });
                let _ = reply.send(out);
            }
            Request::Part {
                path,
                uid_validity,
                uid,
                part,
                reply,
            } => {
                let out = on_session!(
                    self,
                    self.label(&format!("Fetching attachment {part} of {path} #{uid}")),
                    |s| ops::message_part(s, &path, uid_validity, uid, &part)
                );
                let _ = reply.send(out);
            }
            Request::Envelope {
                path,
                uid_validity,
                uid,
                reply,
            } => {
                let out = on_session!(self, self.label(&format!("Reading {path} #{uid}")), |s| {
                    ops::message_envelope(s, &path, uid_validity, uid)
                });
                let _ = reply.send(out);
            }
            Request::AppendToSent { source, reply } => {
                let out = self.file_in_sent(&source).await;
                let _ = reply.send(out);
            }
            Request::AddFlags {
                path,
                uid_validity,
                uid,
                flags,
                reply,
            } => {
                let out = on_session!(
                    self,
                    self.label(&format!("Flagging {path} #{uid} {flags}")),
                    |s| ops::add_flags(s, &path, uid_validity, uid, &flags)
                );
                let _ = reply.send(out);
            }
            Request::Disconnect { reply } => {
                self.close().await;
                let _ = reply.send(Ok(()));
            }
        }
    }

    /// Find the Sent mailbox and put the message in it.
    ///
    /// Two commands and therefore two `on_session!` blocks — the discovery
    /// costs a `LIST` only where the config named no folder, and a server
    /// that advertises no `\Sent` earns an error that says what to write in
    /// the config rather than a silent skip.
    async fn file_in_sent(&mut self, source: &[u8]) -> MailResult<String> {
        let path = match self.account.sent_folder.clone() {
            Some(path) => path,
            None => {
                let found = on_session!(self, self.label("Looking for the Sent folder"), |s| {
                    ops::sent_folder(s)
                })?;
                found.ok_or_else(|| {
                    MailError::Config(format!(
                        "account `{}`: the server marks no Sent folder, so the copy has                          nowhere to go — name one with sent_folder:",
                        self.account.id
                    ))
                })?
            }
        };
        on_session!(self, self.label(&format!("Filing a copy in {path}")), |s| {
            ops::append(s, &path, "(\\Seen)", source)
        })?;
        Ok(path)
    }

    /// The session, connecting first if there is none. The flag says whether
    /// it was built just now — which decides whether a failure is worth a
    /// second attempt.
    async fn take_session(&mut self) -> MailResult<(SessionState, bool)> {
        if let Some(session) = self.session.take() {
            return Ok((session, false));
        }
        // A fresh session has nothing selected, and `SessionState` is what
        // guarantees that: the selection cache is born with the session, so
        // a reconnect cannot inherit the old one's idea of where it is.
        let session = SessionState::new(self.establish_with_retries().await?);
        Ok((session, true))
    }

    /// Open the connection, trying again while the failure is one that goes
    /// away by itself.
    ///
    /// The whole point of the loop is what the user does *not* see: a mail
    /// server that is restarting, or a VPN route that comes up a second after
    /// the tab did, refuses the socket once and accepts it immediately after.
    /// Reporting that as a failed account — a red banner that stays until
    /// something reloads — turns a hiccup into an error the user has to clear
    /// by hand. So `Failed` is published here and only here, after the last
    /// attempt: an intermediate failure names itself in the connect banner
    /// and is then simply tried again.
    ///
    /// What is *not* repeated is anything the server actually answered:
    /// a rejected password (replaying it is how an account gets locked), a
    /// cancelled dialog, a config that cannot describe a connection. See
    /// [`MailError::is_worth_reconnecting`].
    async fn establish_with_retries(&mut self) -> MailResult<MailSession> {
        let who = self.account.label().to_string();
        let attempts = self.retry.attempts.max(1);
        let mut backoff = Duration::from_millis(self.retry.backoff_ms);

        for n in 1..=attempts {
            let attempt = Attempt { n, of: attempts };
            match self.establish(&who, attempt).await {
                Ok(session) => return Ok(session),
                Err(e) if n < attempts && e.is_worth_reconnecting() => {
                    // Named, not swallowed: the banner says what went wrong
                    // and that another try is coming, so a connection that
                    // takes three attempts does not look like a hang.
                    self.status.connect_step(format!(
                        "{who}: {e} — trying again in {:.1}s",
                        backoff.as_secs_f32()
                    ));
                    self.status
                        .connect_attempt(n + 1, attempts, login::CONNECT_TIMEOUT_SECS);
                    tokio::time::sleep(backoff).await;
                    backoff = backoff.saturating_mul(2);
                }
                Err(e) => {
                    self.status.failed(format!("{who}: {e}"));
                    return Err(e);
                }
            }
        }
        // Unreachable: the loop runs at least once and every arm either
        // returns or continues. Stated as an error rather than a panic.
        Err(MailError::Config(format!(
            "{who}: no connection attempt was made"
        )))
    }

    /// One attempt: resolve credentials, open the transport, log in. Reports
    /// each step, so a login that waits on a password store says so instead
    /// of looking hung.
    ///
    /// Publishes no failure of its own — that is
    /// [`Actor::establish_with_retries`]'s call to make, because only it knows
    /// whether this attempt was the last one.
    async fn establish(&mut self, who: &str, attempt: Attempt) -> MailResult<MailSession> {
        self.status.begin_connect();
        // Before the first step, so the counter is on screen for the whole
        // attempt — unlocking a password store is where a login waits
        // longest, and that is the worst moment to look like the first try.
        if attempt.of > 1 {
            self.status.connect_attempt(attempt.n, attempt.of, 0);
        }
        self.status
            .connect_step(format!("{who}: unlocking credentials"));

        let result = async {
            let creds = self.creds.fields().await?;
            let client = login::connect(&self.account, &self.status, who, attempt).await?;
            login::login(
                client,
                &self.account.auth.mechanism,
                &creds,
                &self.status,
                who,
            )
            .await
        }
        .await;

        match result {
            Ok(session) => {
                self.status.connected();
                Ok(session)
            }
            Err(e) => {
                // A rejected password must not be replayed: some servers lock
                // the account after a handful of tries, and the user would
                // never be asked for the right one.
                if e.is_auth() {
                    self.creds.invalidate().await;
                }
                Err(e)
            }
        }
    }

    async fn close(&mut self) {
        if let Some(mut session) = self.session.take() {
            ops::logout(&mut session).await;
        }
    }

    fn label(&self, what: &str) -> String {
        format!("{}: {what}", self.account.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Security;
    use crate::credentials::LoginLane;
    use crate::imap::testserver::{FakeServer, PASSWORD, Script, scripted};
    use not_yet_done_content::{
        AdapterStatus, AuthSpec, CredentialBinding, CredentialProvider, SessionCachePolicy,
    };
    use tokio::sync::watch;

    fn account(port: u16, password: &str) -> Arc<AccountConfig> {
        account_excluding(port, password, &[])
    }

    fn account_excluding(port: u16, password: &str, exclude: &[&str]) -> Arc<AccountConfig> {
        Arc::new(AccountConfig {
            id: "work".into(),
            name: Some("Work".into()),
            address: None,
            host: "127.0.0.1".into(),
            port: Some(port),
            security: Security::None,
            accept_invalid_certs: false,
            default_folder: "INBOX".into(),
            exclude_folders: exclude.iter().map(|s| s.to_string()).collect(),
            auth: literal_auth(password),
            page_size: None,
            retry: None,
            command_timeout_secs: None,
            smtp: None,
            sent_folder: None,
            compose_format: None,
            quote_images: None,
        })
    }

    fn literal_auth(password: &str) -> AuthSpec {
        let literal = |value: &str| CredentialProvider::Literal {
            value: value.to_string(),
        };
        AuthSpec {
            mechanism: "password".into(),
            session_cache: SessionCachePolicy::default(),
            script: None,
            script_timeout_secs: 120,
            bindings: vec![
                CredentialBinding {
                    field: "username".into(),
                    provider: literal("someone"),
                    label: None,
                    masked: None,
                },
                CredentialBinding {
                    field: "password".into(),
                    provider: literal(password),
                    label: None,
                    masked: None,
                },
            ],
        }
    }

    /// A live connection plus its status handles. The receiver is returned
    /// and must be kept: a `watch` channel whose receivers have all been
    /// dropped is closed, and the reporter's updates would go nowhere.
    fn connection(
        account: Arc<AccountConfig>,
    ) -> (Connection, StatusReporter, watch::Receiver<AdapterStatus>) {
        connection_with_timeout(account, Duration::from_secs(60))
    }

    /// The same, with an explicit deadline — for the tests that are about
    /// the deadline itself.
    fn connection_with_timeout(
        account: Arc<AccountConfig>,
        command_timeout: Duration,
    ) -> (Connection, StatusReporter, watch::Receiver<AdapterStatus>) {
        // One attempt: a test that wants a failed connect must not sit
        // through two backoffs to get it.
        connection_with(
            account,
            command_timeout,
            RetryConfig {
                attempts: 1,
                backoff_ms: 0,
            },
        )
    }

    /// The full set of knobs — for the tests about repeating a connect.
    fn connection_with(
        account: Arc<AccountConfig>,
        command_timeout: Duration,
        retry: RetryConfig,
    ) -> (Connection, StatusReporter, watch::Receiver<AdapterStatus>) {
        let status = StatusReporter::new();
        let watching = status.subscribe();
        let creds = AccountCredentials::new(
            account.id.clone(),
            account.auth.clone(),
            status.clone(),
            LoginLane::new(),
        )
        .expect("spec is valid");
        (
            Connection::spawn(
                Arc::clone(&account),
                creds,
                status.clone(),
                command_timeout,
                retry,
            ),
            status,
            watching,
        )
    }

    /// Nothing connects until something is asked for — the reason six
    /// accounts in one instance are affordable at all.
    #[tokio::test]
    async fn nothing_connects_until_a_request_arrives() {
        let server = FakeServer::start(scripted()).await;
        let (conn, status, _watching) = connection(account(server.addr.port(), PASSWORD));
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert_eq!(server.connections(), 0, "spawning must not log in");
        assert!(matches!(status.current(), AdapterStatus::Idle));

        let caps = conn.capabilities().await.expect("logs in and answers");
        assert!(caps.iter().any(|c| c == "IMAP4rev1"), "{caps:?}");
        assert_eq!(server.connections(), 1);
        assert!(matches!(status.current(), AdapterStatus::Ready));
    }

    /// The session is kept: a second request must not log in again.
    #[tokio::test]
    async fn the_session_is_reused() {
        let server = FakeServer::start(scripted()).await;
        let (conn, _status, _watching) = connection(account(server.addr.port(), PASSWORD));
        conn.capabilities().await.expect("first");
        conn.capabilities().await.expect("second");
        assert_eq!(server.connections(), 1, "one connection, two commands");
        assert_eq!(server.logins(), 1);
    }

    /// A server that drops an idle session is routine, not an error: the
    /// second request reconnects and succeeds, and the user sees nothing.
    #[tokio::test]
    async fn a_dead_session_costs_one_reconnect_not_the_request() {
        let server = FakeServer::start(Script {
            // LOGIN is command 1, the first CAPABILITY command 2; the second
            // CAPABILITY (command 3) meets a server that has gone away.
            hang_up_after: Some(3),
            ..scripted()
        })
        .await;
        let (conn, status, _watching) = connection(account(server.addr.port(), PASSWORD));
        conn.capabilities().await.expect("first request");

        let caps = conn.capabilities().await.expect("survives the hang-up");
        assert!(caps.iter().any(|c| c == "IMAP4rev1"), "{caps:?}");
        assert_eq!(server.connections(), 2, "exactly one reconnect");
        assert!(
            !matches!(status.current(), AdapterStatus::Failed { .. }),
            "a recovered timeout is not a failure the user should see"
        );
    }

    /// A server that answers nothing and closes nothing is the case the
    /// deadline exists for: without one the actor waits forever, and because
    /// IMAP runs one command at a time, so does everything queued behind it.
    #[tokio::test]
    async fn a_mute_server_costs_the_deadline_and_not_the_session_forever() {
        let server = FakeServer::start(Script {
            // LOGIN is command 1, the first CAPABILITY command 2; the second
            // CAPABILITY (command 3) is read and never answered.
            stall_at: Some(3),
            ..scripted()
        })
        .await;
        let (conn, status, _watching) = connection_with_timeout(
            account(server.addr.port(), PASSWORD),
            Duration::from_millis(200),
        );
        conn.capabilities().await.expect("first request");

        let started = std::time::Instant::now();
        let err = conn.capabilities().await.expect_err("nothing ever answers");
        assert!(
            matches!(err, MailError::Timeout(_)),
            "a silent server is a timeout, not a lost connection: {err:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the wait is bounded by the deadline, not by the network stack"
        );
        // The reconnect exists for a session the *server* ended. Repeating a
        // command that ran out of time would only make the user wait twice.
        assert_eq!(server.connections(), 1, "a deadline is not worth a retry");
        assert!(
            matches!(status.current(), AdapterStatus::Failed { .. }),
            "the user is told why the request stopped"
        );
    }

    /// The queue behind a stalled request must drain too: the deadline frees
    /// the actor, so the next request gets a fresh session and an answer.
    #[tokio::test]
    async fn the_queue_behind_a_stall_still_gets_served() {
        let server = FakeServer::start(Script {
            stall_at: Some(3),
            ..scripted()
        })
        .await;
        let (conn, _status, _watching) = connection_with_timeout(
            account(server.addr.port(), PASSWORD),
            Duration::from_millis(200),
        );
        conn.capabilities().await.expect("first request");
        conn.capabilities()
            .await
            .expect_err("runs into the deadline");

        let caps = conn
            .capabilities()
            .await
            .expect("the account is usable again");
        assert!(caps.iter().any(|c| c == "IMAP4rev1"), "{caps:?}");
        assert_eq!(
            server.connections(),
            2,
            "the stalled session was thrown away"
        );
    }

    /// The failure the retry exists for: a server that is not up *yet*. The
    /// user asked for a folder list and gets one — the hiccup never becomes
    /// a red banner they have to clear.
    #[tokio::test]
    async fn a_connection_that_is_refused_once_is_simply_opened_again() {
        let server = FakeServer::start(Script {
            close_first: 1,
            ..scripted()
        })
        .await;
        let (conn, status, _watching) = connection_with(
            account(server.addr.port(), PASSWORD),
            Duration::from_secs(60),
            RetryConfig {
                attempts: 3,
                backoff_ms: 1,
            },
        );

        let folders = conn.folders().await.expect("the second attempt gets in");
        assert!(!folders.is_empty());
        assert_eq!(server.connections(), 2, "one refusal, one retry");
        assert!(
            !matches!(status.current(), AdapterStatus::Failed { .. }),
            "a connection that came up on the second try is not a failure"
        );
    }

    /// And it gives up: `attempts` is a bound, not a loop. After the last one
    /// the user is told, once.
    #[tokio::test]
    async fn a_server_that_stays_down_fails_after_the_last_attempt() {
        let server = FakeServer::start(Script {
            close_first: 10,
            ..scripted()
        })
        .await;
        let (conn, status, _watching) = connection_with(
            account(server.addr.port(), PASSWORD),
            Duration::from_secs(60),
            RetryConfig {
                attempts: 2,
                backoff_ms: 1,
            },
        );

        conn.folders().await.expect_err("nobody is answering");
        assert_eq!(server.connections(), 2, "exactly `attempts` attempts");
        match status.current() {
            AdapterStatus::Failed { reason } => {
                assert!(reason.contains("Work"), "names the account: {reason}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    /// What must *not* be repeated. Some servers lock an account after a
    /// handful of refused logins, so a wrong password is asked about once —
    /// however many attempts the config allows for a dead socket.
    #[tokio::test]
    async fn a_refused_password_is_never_replayed() {
        let server = FakeServer::start(scripted()).await;
        let (conn, _status, _watching) = connection_with(
            account(server.addr.port(), "wrong"),
            Duration::from_secs(60),
            RetryConfig {
                attempts: 3,
                backoff_ms: 1,
            },
        );

        let err = conn.capabilities().await.expect_err("the server says no");
        assert!(err.is_auth(), "{err}");
        assert_eq!(server.logins(), 1, "one LOGIN, not three");
    }

    /// A refused password must surface as an auth error — that is what makes
    /// the adapter drop the credential and ask again rather than replay it.
    #[tokio::test]
    async fn a_refused_password_is_an_auth_error() {
        let server = FakeServer::start(scripted()).await;
        let (conn, status, _watching) = connection(account(server.addr.port(), "wrong"));
        let err = conn.capabilities().await.expect_err("the server says no");
        assert!(err.is_auth(), "{err}");
        assert!(err.to_string().contains("Invalid credentials"), "{err}");
        match status.current() {
            AdapterStatus::Failed { reason } => {
                assert!(reason.contains("Work"), "names the account: {reason}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    /// The folder list is what the tree is built from: decoded labels,
    /// `INBOX` first, and `\\Noselect` parents kept as rows but marked, since
    /// selecting one is an error.
    #[tokio::test]
    async fn folders_come_back_decoded_and_in_display_order() {
        let server = FakeServer::start(scripted()).await;
        let (conn, _status, _watching) = connection(account(server.addr.port(), PASSWORD));

        let folders = conn.folders().await.expect("lists");
        let paths: Vec<&str> = folders.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "INBOX",
                "Archive",
                "Archive/2019",
                "Entw&APw-rfe",
                "Sent Items"
            ]
        );
        let labels: Vec<&str> = folders.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            labels,
            ["INBOX", "Archive", "2019", "Entwürfe", "Sent Items"],
            "the label is decoded and stripped to its last segment"
        );
        assert!(!folders[1].selectable, "Archive is \\Noselect");
        assert_eq!(folders[2].parent_path(), Some("Archive"));
    }

    /// `exclude_folders` is what keeps a server-side archive of hundreds of
    /// folders out of the tree — and it hides the subtree, not just its root.
    #[tokio::test]
    async fn excluded_folders_never_reach_the_caller() {
        let server = FakeServer::start(scripted()).await;
        let (conn, _status, _watching) = connection(account_excluding(
            server.addr.port(),
            PASSWORD,
            &["Archive/*", "Sent Items"],
        ));

        let folders = conn.folders().await.expect("lists");
        let paths: Vec<&str> = folders.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["INBOX", "Entw&APw-rfe"]);
    }

    /// Counts come from `STATUS`, where `unseen` is the *number* of unread
    /// messages — and the mailbox name has to reach the server quoted, or a
    /// folder with a space in its name is simply unreadable.
    #[tokio::test]
    async fn status_reports_total_and_unread_for_a_named_mailbox() {
        let server = FakeServer::start(scripted()).await;
        let (conn, _status, _watching) = connection(account(server.addr.port(), PASSWORD));

        assert_eq!(conn.folder_status("INBOX").await.expect("INBOX"), (3, 1));
        assert_eq!(
            conn.folder_status("Sent Items").await.expect("quoted name"),
            (7, 0)
        );
        let err = conn
            .folder_status("Nope")
            .await
            .expect_err("no such mailbox");
        assert!(
            !err.is_fatal(),
            "a refused command must not cost the session: {err}"
        );
    }

    /// The message level end to end: search, window, envelopes — and the
    /// decoding that makes the rows readable at all.
    #[tokio::test]
    async fn a_page_of_messages_comes_back_newest_first_and_decoded() {
        let server = FakeServer::start(scripted()).await;
        let (conn, _status, _watching) = connection(account(server.addr.port(), PASSWORD));

        let page = conn.messages("INBOX", "", 0, 50).await.expect("lists");
        assert_eq!(page.total, 3);
        assert_eq!(page.uid_validity, 42);
        // Descending UID is "newest first" without a SORT round trip.
        let uids: Vec<u32> = page.rows.iter().map(|r| r.uid).collect();
        assert_eq!(uids, [9, 4, 1]);

        let oldest = page.rows.last().expect("three rows");
        assert_eq!(oldest.subject, "Grüße", "the subject arrives encoded");
        assert_eq!(oldest.from, "Jürgen <juergen@example.org>");
        assert!(oldest.seen);
        assert_eq!(oldest.size, 1024);

        let invoice = &page.rows[1];
        assert!(!invoice.seen, "no \\Seen flag means unread");
        assert_eq!(
            invoice.attachments.len(),
            1,
            "the PDF part of the multipart is an attachment"
        );
        // The whole part description is kept, not just its count: it is what
        // the attachment level lists, and it arrived with this fetch.
        assert_eq!(invoice.attachments[0].filename, "invoice.pdf");
        assert_eq!(invoice.attachments[0].part, "2");
        assert_eq!(invoice.attachments[0].content_type, "application/pdf");
        // The Date header, not the server's arrival time: the fixture gives
        // them deliberately different days.
        assert_eq!(
            invoice.date.expect("has a date").to_rfc3339(),
            "2026-09-02T10:30:00+02:00"
        );
    }

    /// Paging is a window over the search result, not a second search: the
    /// total stays the whole match while the rows are only the slice asked
    /// for. That is what makes a folder of a hundred thousand affordable.
    #[tokio::test]
    async fn a_window_fetches_only_its_slice() {
        let server = FakeServer::start(scripted()).await;
        let (conn, _status, _watching) = connection(account(server.addr.port(), PASSWORD));

        let page = conn.messages("INBOX", "", 1, 1).await.expect("lists");
        assert_eq!(page.total, 3, "the total is the whole match");
        let uids: Vec<u32> = page.rows.iter().map(|r| r.uid).collect();
        assert_eq!(uids, [4], "one row, the second-newest");

        let past_the_end = conn.messages("INBOX", "", 99, 50).await.expect("lists");
        assert!(past_the_end.rows.is_empty());
        assert_eq!(past_the_end.total, 3);
    }

    /// A mailbox that cannot be opened must fail as a *refused command*, not
    /// as a dead session — otherwise one mistyped folder costs a reconnect.
    #[tokio::test]
    async fn selecting_a_missing_mailbox_does_not_cost_the_session() {
        let server = FakeServer::start(scripted()).await;
        let (conn, _status, _watching) = connection(account(server.addr.port(), PASSWORD));
        conn.messages("INBOX", "", 0, 50).await.expect("first");

        let err = conn
            .messages("Nope", "", 0, 50)
            .await
            .expect_err("no such mailbox");
        assert!(!err.is_fatal(), "{err}");
        conn.messages("INBOX", "", 0, 50).await.expect("still live");
        assert_eq!(server.connections(), 1, "no reconnect");
    }

    /// A body is the message as it travelled — headers, encodings and all.
    /// Decoding it is the MIME layer's job, not the protocol layer's, so
    /// what has to arrive here is the source verbatim.
    #[tokio::test]
    async fn a_body_arrives_as_the_message_travelled() {
        let server = FakeServer::start(scripted()).await;
        let (conn, _status, _watching) = connection(account(server.addr.port(), PASSWORD));

        let source = conn.body("INBOX", 42, 1).await.expect("reads");
        let text = String::from_utf8_lossy(&source);
        assert!(
            text.contains("Content-Transfer-Encoding: quoted-printable"),
            "the source is not decoded on the way: {text}"
        );
        assert!(text.contains("Gr=FC=DFe aus M=FCnchen"), "{text}");
    }

    /// A part comes back *decoded*, because the client asked for the part's
    /// MIME headers alongside it. Without them this would be the base64 text
    /// rather than the file.
    #[tokio::test]
    async fn a_part_arrives_decoded_because_its_headers_came_with_it() {
        let server = FakeServer::start(scripted()).await;
        let (conn, _status, _watching) = connection(account(server.addr.port(), PASSWORD));

        let bytes = conn.part("INBOX", 42, 4, "2").await.expect("fetches");
        assert_eq!(bytes, b"hello world", "base64 was decoded, not saved");
        assert_eq!(
            server.fetches(),
            1,
            "headers and body are one command, not two"
        );
    }

    /// The UIDVALIDITY in a message id is a *check*, not decoration: a
    /// renumbered mailbox has to fail loudly rather than open whatever mail
    /// now wears that UID. And it must not cost the session.
    #[tokio::test]
    async fn a_renumbered_mailbox_refuses_an_old_id_instead_of_opening_the_wrong_mail() {
        let server = FakeServer::start(scripted()).await;
        let (conn, _status, _watching) = connection(account(server.addr.port(), PASSWORD));

        let err = conn
            .body("INBOX", 41, 1)
            .await
            .expect_err("the id is from before the renumbering");
        assert!(err.to_string().contains("renumbered"), "{err}");
        assert!(!err.is_fatal(), "the session is fine: {err}");
        conn.body("INBOX", 42, 1).await.expect("still live");
    }

    /// One envelope for one UID — the path a message reached without its
    /// listing takes.
    #[tokio::test]
    async fn a_single_envelope_can_be_fetched_without_its_page() {
        let server = FakeServer::start(scripted()).await;
        let (conn, _status, _watching) = connection(account(server.addr.port(), PASSWORD));

        let row = conn.envelope("INBOX", 42, 4).await.expect("fetches");
        assert_eq!(row.uid, 4);
        assert_eq!(row.subject, "Rechnung");
        assert_eq!(row.attachments.len(), 1);

        let err = conn
            .envelope("INBOX", 42, 4711)
            .await
            .expect_err("no such message");
        assert!(err.to_string().contains("moved or deleted"), "{err}");
    }

    /// The copy in Sent, without a word of configuration: the server marks
    /// a mailbox `\Sent` and that is the one the message lands in, with the
    /// flag that keeps it from showing up as unread mail of one's own.
    #[tokio::test]
    async fn a_sent_message_is_filed_in_the_folder_the_server_marks() {
        let server = FakeServer::start(scripted()).await;
        let (conn, _status, _watching) = connection(account(server.addr.port(), PASSWORD));

        let folder = conn
            .append_to_sent(b"From: me@example.invalid\r\nSubject: hi\r\n\r\nbody\r\n".to_vec())
            .await
            .expect("files the copy");
        assert_eq!(folder, "Sent Items");

        let filed = server.appended();
        assert_eq!(filed.len(), 1);
        assert_eq!(filed[0].folder, "Sent Items");
        assert_eq!(filed[0].flags, "\\Seen");
        assert!(filed[0].source.contains("Subject: hi"), "{:?}", filed[0]);
    }

    /// A configured folder is not a suggestion: it is used, and the `LIST`
    /// that would have looked for one never happens.
    #[tokio::test]
    async fn a_configured_sent_folder_wins_over_the_servers_own() {
        let server = FakeServer::start(scripted()).await;
        let mut account = account(server.addr.port(), PASSWORD);
        Arc::get_mut(&mut account).expect("sole owner").sent_folder = Some("Archive".into());
        let (conn, _status, _watching) = connection(account);

        let folder = conn
            .append_to_sent(b"Subject: hi\r\n\r\nbody\r\n".to_vec())
            .await
            .expect("files the copy");
        assert_eq!(folder, "Archive");
        assert_eq!(server.lists(), 0, "no discovery was needed");
    }

    /// The ↩ glyph in the message list is this command.
    #[tokio::test]
    async fn answering_a_message_flags_it() {
        let server = FakeServer::start(scripted()).await;
        let (conn, _status, _watching) = connection(account(server.addr.port(), PASSWORD));

        conn.add_flags("INBOX", 42, 4, "\\Answered")
            .await
            .expect("flags it");
        let stored = server.stored();
        assert_eq!(stored.len(), 1);
        assert!(
            stored[0].contains("UID STORE 4 +FLAGS.SILENT (\\Answered)"),
            "{}",
            stored[0]
        );
    }

    /// A mailbox that was renumbered under us must not be flagged by uid:
    /// the same number now addresses a different message.
    #[tokio::test]
    async fn a_flag_is_refused_when_the_folder_was_renumbered() {
        let server = FakeServer::start(scripted()).await;
        let (conn, _status, _watching) = connection(account(server.addr.port(), PASSWORD));

        let err = conn
            .add_flags("INBOX", 41, 4, "\\Answered")
            .await
            .expect_err("stale uid validity");
        assert!(err.to_string().contains("renumbered"), "{err}");
        assert!(server.stored().is_empty(), "nothing was flagged");
    }

    /// `disconnect` really ends the session; the next request builds a new
    /// one. This is the manual-reconnect path.
    #[tokio::test]
    async fn disconnect_ends_the_session_and_the_next_request_rebuilds_it() {
        let server = FakeServer::start(scripted()).await;
        let (conn, _status, _watching) = connection(account(server.addr.port(), PASSWORD));
        conn.capabilities().await.expect("first");
        conn.disconnect().await.expect("logs out");
        conn.capabilities().await.expect("second");
        assert_eq!(server.connections(), 2);
        assert_eq!(server.logins(), 2);
    }
}
