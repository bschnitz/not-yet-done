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

/// How the server behaves.
#[derive(Clone, Copy, Default)]
pub(crate) struct Script {
    /// The password it accepts. Anything else is answered with `NO`.
    pub(crate) password: &'static str,
    /// Hang up on the *first* connection after this many commands, without
    /// answering the last one — a server-side idle timeout, as the client
    /// experiences it.
    pub(crate) hang_up_after: Option<usize>,
}

pub(crate) struct FakeServer {
    pub(crate) addr: SocketAddr,
    connections: Arc<AtomicUsize>,
    logins: Arc<AtomicUsize>,
}

impl FakeServer {
    pub(crate) async fn start(script: Script) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
        let addr = listener.local_addr().expect("has an address");
        let connections = Arc::new(AtomicUsize::new(0));
        let logins = Arc::new(AtomicUsize::new(0));
        let (conns, logs) = (Arc::clone(&connections), Arc::clone(&logins));
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    return;
                };
                let nth = conns.fetch_add(1, Ordering::SeqCst);
                let logs = Arc::clone(&logs);
                tokio::spawn(async move { serve(sock, script, nth, logs).await });
            }
        });
        Self {
            addr,
            connections,
            logins,
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
}

async fn serve(sock: tokio::net::TcpStream, script: Script, nth: usize, logins: Arc<AtomicUsize>) {
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
        let reply = match cmd.as_str() {
            "LOGIN" => {
                logins.fetch_add(1, Ordering::SeqCst);
                if args.get(1).map(String::as_str) == Some(script.password) {
                    format!("{tag} OK LOGIN completed\r\n")
                } else {
                    format!("{tag} NO [AUTHENTICATIONFAILED] Invalid credentials\r\n")
                }
            }
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
