//! A fake IMAP server, just big enough to log in and answer `CAPABILITY`.
//!
//! The connection actor's interesting behaviour — lazy connect, one
//! reconnect when a *reused* session has died, dropping the credentials when
//! the server refuses them — is lifecycle, not protocol. A scripted server on
//! loopback exercises it end to end without a network or an account.

#![cfg(test)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// One mailbox the fake server knows about.
pub(crate) struct FakeFolder {
    /// The name on the wire — modified UTF-7 and all.
    pub(crate) path: &'static str,
    /// Hierarchy delimiter reported for it, or `NIL` when empty.
    pub(crate) delimiter: &'static str,
    /// `LIST` attributes, e.g. `\\Noselect`.
    pub(crate) attributes: &'static str,
    pub(crate) messages: u32,
    pub(crate) unseen: u32,
}

/// How the server behaves.
#[derive(Clone, Copy, Default)]
pub(crate) struct Script {
    /// The password it accepts. Anything else is answered with `NO`.
    pub(crate) password: &'static str,
    /// Hang up on the *first* connection after this many commands, without
    /// answering the last one — a server-side idle timeout, as the client
    /// experiences it.
    pub(crate) hang_up_after: Option<usize>,
    /// What `LIST` and `STATUS` report. Empty means the account has no
    /// mailboxes at all, which is itself worth being able to script.
    pub(crate) folders: &'static [FakeFolder],
}

/// The password the scripted server accepts.
pub(crate) const PASSWORD: &str = "right";

/// A little of everything a real server throws at the folder layer: a
/// `\\Noselect` parent, a name in modified UTF-7, and one with a space in it
/// (which only survives if the client quotes properly).
pub(crate) const FOLDERS: &[FakeFolder] = &[
    FakeFolder {
        path: "INBOX",
        delimiter: "/",
        attributes: "\\HasNoChildren",
        messages: 12,
        unseen: 3,
    },
    FakeFolder {
        path: "Archive",
        delimiter: "/",
        attributes: "\\Noselect \\HasChildren",
        messages: 0,
        unseen: 0,
    },
    FakeFolder {
        path: "Archive/2019",
        delimiter: "/",
        attributes: "\\HasNoChildren",
        messages: 4,
        unseen: 0,
    },
    FakeFolder {
        path: "Entw&APw-rfe",
        delimiter: "/",
        attributes: "\\Drafts",
        messages: 1,
        unseen: 0,
    },
    FakeFolder {
        path: "Sent Items",
        delimiter: "/",
        attributes: "\\Sent",
        messages: 7,
        unseen: 0,
    },
];

/// The ordinary server: the right password and the fixture mailboxes.
pub(crate) fn scripted() -> Script {
    Script {
        password: PASSWORD,
        hang_up_after: None,
        folders: FOLDERS,
    }
}

pub(crate) struct FakeServer {
    pub(crate) addr: SocketAddr,
    connections: Arc<AtomicUsize>,
    logins: Arc<AtomicUsize>,
    lists: Arc<AtomicUsize>,
}

impl FakeServer {
    pub(crate) async fn start(script: Script) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
        let addr = listener.local_addr().expect("has an address");
        let connections = Arc::new(AtomicUsize::new(0));
        let logins = Arc::new(AtomicUsize::new(0));
        let lists = Arc::new(AtomicUsize::new(0));
        let (conns, logs, lsts) = (
            Arc::clone(&connections),
            Arc::clone(&logins),
            Arc::clone(&lists),
        );
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    return;
                };
                let nth = conns.fetch_add(1, Ordering::SeqCst);
                let (logs, lsts) = (Arc::clone(&logs), Arc::clone(&lsts));
                tokio::spawn(async move { serve(sock, script, nth, logs, lsts).await });
            }
        });
        Self {
            addr,
            connections,
            logins,
            lists,
        }
    }

    /// How many TCP connections the client has opened — the reconnect count.
    pub(crate) fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    /// How many `LOGIN` commands arrived, successful or not.
    pub(crate) fn logins(&self) -> usize {
        self.logins.load(Ordering::SeqCst)
    }

    /// How many `LIST` commands arrived — what says whether the folder
    /// snapshot was reused or re-fetched.
    pub(crate) fn lists(&self) -> usize {
        self.lists.load(Ordering::SeqCst)
    }
}

async fn serve(
    sock: tokio::net::TcpStream,
    script: Script,
    nth: usize,
    logins: Arc<AtomicUsize>,
    lists: Arc<AtomicUsize>,
) {
    let (rx, mut tx) = sock.into_split();
    let mut lines = BufReader::new(rx).lines();
    if tx
        .write_all(b"* OK [CAPABILITY IMAP4rev1] fake ready\r\n")
        .await
        .is_err()
    {
        return;
    }
    let mut commands = 0usize;
    while let Ok(Some(line)) = lines.next_line().await {
        commands += 1;
        if nth == 0 && script.hang_up_after == Some(commands) {
            // Gone without a word — exactly what a timed-out session looks
            // like from the client side.
            return;
        }
        let mut parts = line.split_whitespace();
        let tag = parts.next().unwrap_or("*").to_string();
        let cmd = parts.next().unwrap_or("").to_ascii_uppercase();
        let args: Vec<String> = parts.map(|s| s.trim_matches('"').to_string()).collect();
        // A mailbox name may contain spaces, so it is read from the quotes
        // rather than from the whitespace split — which is exactly the
        // quoting the client has to get right.
        let quoted =
            |nth: usize| -> Option<String> { line.split('"').nth(nth * 2 + 1).map(str::to_string) };
        let reply = match cmd.as_str() {
            "LOGIN" => {
                logins.fetch_add(1, Ordering::SeqCst);
                if args.get(1).map(String::as_str) == Some(script.password) {
                    format!("{tag} OK LOGIN completed\r\n")
                } else {
                    format!("{tag} NO [AUTHENTICATIONFAILED] Invalid credentials\r\n")
                }
            }
            "LIST" => {
                lists.fetch_add(1, Ordering::SeqCst);
                let mut out = String::new();
                for f in script.folders {
                    let delim = if f.delimiter.is_empty() {
                        "NIL".to_string()
                    } else {
                        format!("\"{}\"", f.delimiter)
                    };
                    out.push_str(&format!(
                        "* LIST ({}) {delim} \"{}\"\r\n",
                        f.attributes, f.path
                    ));
                }
                out.push_str(&format!("{tag} OK LIST completed\r\n"));
                out
            }
            "STATUS" => match quoted(0).and_then(|name| {
                script
                    .folders
                    .iter()
                    .find(|f| f.path == name)
                    .map(|f| (name, f))
            }) {
                Some((name, f)) => format!(
                    "* STATUS \"{name}\" (MESSAGES {} UNSEEN {})\r\n{tag} OK STATUS completed\r\n",
                    f.messages, f.unseen
                ),
                None => format!("{tag} NO [NONEXISTENT] no such mailbox\r\n"),
            },
            "CAPABILITY" => {
                format!("* CAPABILITY IMAP4rev1 IDLE MOVE\r\n{tag} OK CAPABILITY completed\r\n")
            }
            "LOGOUT" => {
                let _ = tx
                    .write_all(format!("* BYE\r\n{tag} OK LOGOUT completed\r\n").as_bytes())
                    .await;
                return;
            }
            other => format!("{tag} BAD unknown command {other}\r\n"),
        };
        if tx.write_all(reply.as_bytes()).await.is_err() {
            return;
        }
    }
}
