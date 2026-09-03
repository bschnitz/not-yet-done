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
//! Adding a command means: a function in [`super::ops`], a variant in
//! [`Request`], one arm in [`Actor::serve`], and a method on [`Connection`].

use std::sync::Arc;

use not_yet_done_content::StatusReporter;
use tokio::sync::{mpsc, oneshot};

use super::login::{self, MailSession};
use super::ops::{self, MessagePage};
use super::session::SessionState;
use crate::config::AccountConfig;
use crate::credentials::AccountCredentials;
use crate::error::{MailError, MailResult};
use crate::model::FolderInfo;

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
    ) -> Self {
        let (tx, rx) = mpsc::channel(QUEUE_DEPTH);
        let actor = Actor {
            account,
            creds,
            status,
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
    session: Option<SessionState>,
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
            let announced = $self.status.busy($label, 0);
            let outcome = {
                let $s: &mut SessionState = &mut owned;
                $call.await
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
                    if fresh {
                        // It failed on a session we had just built — trying
                        // again would only repeat it.
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
            Request::Disconnect { reply } => {
                self.close().await;
                let _ = reply.send(Ok(()));
            }
        }
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
        let session = SessionState::new(self.establish().await?);
        Ok((session, true))
    }

    /// Resolve credentials, open the transport, log in. Reports each step, so
    /// a login that waits on a password store says so instead of looking
    /// hung.
    async fn establish(&mut self) -> MailResult<MailSession> {
        let who = self.account.label().to_string();
        self.status.begin_connect();
        self.status
            .connect_step(format!("{who}: unlocking credentials"));

        let result = async {
            let creds = self.creds.fields().await?;
            let client = login::connect(&self.account, &self.status, &who).await?;
            login::login(
                client,
                &self.account.auth.mechanism,
                &creds,
                &self.status,
                &who,
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
                self.status.failed(format!("{who}: {e}"));
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
            Connection::spawn(Arc::clone(&account), creds, status.clone()),
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
            invoice.attachments, 1,
            "the PDF part of the multipart is an attachment"
        );
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
