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
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// One message the fake server holds.
pub(crate) struct FakeMessage {
    pub(crate) uid: u32,
    /// The `ENVELOPE` fields, already in wire form — subject and display name
    /// encoded exactly as a real server hands them over.
    pub(crate) subject: &'static str,
    pub(crate) from_name: &'static str,
    pub(crate) from_mailbox: &'static str,
    pub(crate) from_host: &'static str,
    pub(crate) date: &'static str,
    pub(crate) size: u32,
    pub(crate) flags: &'static str,
    /// `true` gives the message a multipart body with one attachment.
    pub(crate) attachment: bool,
    /// What `BODY[]` hands back — the message as it travelled.
    pub(crate) source: &'static str,
    /// What `BODY[<section>]` hands back, as
    /// `(section, MIME headers, body)`. The headers are a section of their
    /// own on the wire (`BODY[2.MIME]`) and carry the transfer encoding, so
    /// a client that forgets them writes base64 to disk.
    pub(crate) parts: &'static [(&'static str, &'static str, &'static str)],
}

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
    /// UIDVALIDITY reported by `SELECT`; half of every message id.
    pub(crate) uid_validity: u32,
    pub(crate) mails: &'static [FakeMessage],
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
    /// Go silent on the *first* connection at this command: read it, answer
    /// nothing, and hold the socket open. A hung-up server and a mute one
    /// look the same to a user and completely different to a client — the
    /// first ends the read, the second ends nothing at all, which is the
    /// case a deadline exists for.
    pub(crate) stall_at: Option<usize>,
    /// Accept and drop the first N connections without so much as a
    /// greeting. What a server that is restarting looks like from the
    /// client's side, and the case the connect loop exists for. Counted, so
    /// a test can say how many attempts it took.
    pub(crate) close_first: usize,
    /// What `LIST` and `STATUS` report. Empty means the account has no
    /// mailboxes at all, which is itself worth being able to script.
    pub(crate) folders: &'static [FakeFolder],
}

/// The password the scripted server accepts.
pub(crate) const PASSWORD: &str = "right";

/// A little of everything the message layer has to survive: a subject and a
/// display name in RFC 2047, a mail with no subject at all, one with an
/// attachment, and flags that are not all the same.
pub(crate) const MAILS: &[FakeMessage] = &[
    FakeMessage {
        uid: 1,
        subject: "=?UTF-8?Q?Gr=C3=BC=C3=9Fe?=",
        from_name: "=?UTF-8?Q?J=C3=BCrgen?=",
        from_mailbox: "juergen",
        from_host: "example.org",
        date: "Tue, 01 Sep 2026 09:00:00 +0200",
        size: 1024,
        flags: "\\Seen",
        attachment: false,
        source: PLAIN_SOURCE,
        parts: &[],
    },
    FakeMessage {
        uid: 4,
        subject: "Rechnung",
        from_name: "",
        from_mailbox: "billing",
        from_host: "example.net",
        date: "Wed, 02 Sep 2026 10:30:00 +0200",
        size: 40960,
        flags: "",
        attachment: true,
        source: MIXED_SOURCE,
        parts: MIXED_PARTS,
    },
    FakeMessage {
        uid: 9,
        subject: "",
        from_name: "",
        from_mailbox: "noreply",
        from_host: "example.com",
        date: "Thu, 03 Sep 2026 08:15:00 +0200",
        size: 512,
        flags: "\\Seen \\Answered",
        attachment: false,
        source: HTML_SOURCE,
        parts: &[],
    },
];

/// A one-part message, quoted-printable and not UTF-8 — the ordinary case
/// the body renderer has to survive.
pub(crate) const PLAIN_SOURCE: &str = concat!(
    "From: juergen@example.org\r\n",
    "Subject: =?UTF-8?Q?Gr=C3=BC=C3=9Fe?=\r\n",
    "MIME-Version: 1.0\r\n",
    "Content-Type: text/plain; charset=iso-8859-1\r\n",
    "Content-Transfer-Encoding: quoted-printable\r\n",
    "\r\n",
    "Gr=FC=DFe aus M=FCnchen\r\n"
);

/// The half of every mailbox a text pane cannot show: markup, with the
/// sender's own image carried alongside it and referenced by `cid:`. Its
/// `BODYSTRUCTURE` says `attachment: false` — an inline logo is body, not a
/// file anybody lists, which is exactly why exporting it needs its own seam.
pub(crate) const HTML_SOURCE: &str = concat!(
    "From: news@example.com\r\n",
    "Subject: Newsletter\r\n",
    "MIME-Version: 1.0\r\n",
    "Content-Type: multipart/related; boundary=\"r\"\r\n",
    "\r\n",
    "--r\r\n",
    "Content-Type: text/html; charset=utf-8\r\n",
    "\r\n",
    "<h1>Neues</h1><img src=\"cid:logo@example\">\r\n",
    "--r\r\n",
    "Content-Type: image/png\r\n",
    "Content-ID: <logo@example>\r\n",
    "Content-Transfer-Encoding: base64\r\n",
    "\r\n",
    "aGVsbG8gd29ybGQ=\r\n",
    "--r--\r\n"
);

/// A `multipart/mixed`: text and one base64 attachment.
pub(crate) const MIXED_SOURCE: &str = concat!(
    "From: billing@example.net\r\n",
    "Subject: Rechnung\r\n",
    "MIME-Version: 1.0\r\n",
    "Content-Type: multipart/mixed; boundary=\"b\"\r\n",
    "\r\n",
    "--b\r\n",
    "Content-Type: text/plain; charset=utf-8\r\n",
    "\r\n",
    "Die Rechnung haengt an.\r\n",
    "--b\r\n",
    "Content-Type: application/pdf; name=\"invoice.pdf\"\r\n",
    "Content-Transfer-Encoding: base64\r\n",
    "\r\n",
    "aGVsbG8gd29ybGQ=\r\n",
    "--b--\r\n"
);

/// The two sections of [`MIXED_SOURCE`], as the server hands them out one
/// at a time.
pub(crate) const MIXED_PARTS: &[(&str, &str, &str)] = &[
    (
        "1",
        "Content-Type: text/plain; charset=utf-8\r\n",
        "Die Rechnung haengt an.\r\n",
    ),
    (
        "2",
        concat!(
            "Content-Type: application/pdf; name=\"invoice.pdf\"\r\n",
            "Content-Transfer-Encoding: base64\r\n"
        ),
        "aGVsbG8gd29ybGQ=\r\n",
    ),
];

/// A little of everything a real server throws at the folder layer: a
/// `\\Noselect` parent, a name in modified UTF-7, and one with a space in it
/// (which only survives if the client quotes properly).
pub(crate) const FOLDERS: &[FakeFolder] = &[
    FakeFolder {
        path: "INBOX",
        delimiter: "/",
        attributes: "\\HasNoChildren",
        messages: 3,
        unseen: 1,
        uid_validity: 42,
        mails: MAILS,
    },
    FakeFolder {
        path: "Archive",
        delimiter: "/",
        attributes: "\\Noselect \\HasChildren",
        messages: 0,
        unseen: 0,
        uid_validity: 0,
        mails: &[],
    },
    FakeFolder {
        path: "Archive/2019",
        delimiter: "/",
        attributes: "\\HasNoChildren",
        messages: 4,
        unseen: 0,
        uid_validity: 43,
        mails: &[],
    },
    FakeFolder {
        path: "Entw&APw-rfe",
        delimiter: "/",
        attributes: "\\Drafts",
        messages: 1,
        unseen: 0,
        uid_validity: 44,
        mails: &[],
    },
    FakeFolder {
        path: "Sent Items",
        delimiter: "/",
        attributes: "\\Sent",
        messages: 7,
        unseen: 0,
        uid_validity: 45,
        mails: &[],
    },
];

/// The ordinary server: the right password and the fixture mailboxes.
pub(crate) fn scripted() -> Script {
    Script {
        password: PASSWORD,
        hang_up_after: None,
        stall_at: None,
        close_first: 0,
        folders: FOLDERS,
    }
}

/// One message the client filed with `APPEND`.
#[derive(Clone, Debug)]
pub(crate) struct Appended {
    pub(crate) folder: String,
    /// The flag list, without its parentheses.
    pub(crate) flags: String,
    pub(crate) source: String,
}

/// What the server counted. Every claim about *cost* — one reconnect, no
/// second `LIST`, a body fetched once — is a claim about one of these.
///
/// The write side records what arrived rather than how often: an `APPEND`
/// that put the wrong bytes in the right folder is the failure worth
/// catching, and a count cannot see it.
#[derive(Default)]
struct Counters {
    connections: AtomicUsize,
    logins: AtomicUsize,
    lists: AtomicUsize,
    fetches: AtomicUsize,
    appended: std::sync::Mutex<Vec<Appended>>,
    stored: std::sync::Mutex<Vec<String>>,
}

pub(crate) struct FakeServer {
    pub(crate) addr: SocketAddr,
    counters: Arc<Counters>,
}

impl FakeServer {
    pub(crate) async fn start(script: Script) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
        let addr = listener.local_addr().expect("has an address");
        let counters = Arc::new(Counters::default());
        let accepting = Arc::clone(&counters);
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    return;
                };
                let nth = accepting.connections.fetch_add(1, Ordering::SeqCst);
                let counters = Arc::clone(&accepting);
                tokio::spawn(async move { serve(sock, script, nth, counters).await });
            }
        });
        Self { addr, counters }
    }

    /// How many TCP connections the client has opened — the reconnect count.
    pub(crate) fn connections(&self) -> usize {
        self.counters.connections.load(Ordering::SeqCst)
    }

    /// How many `LOGIN` commands arrived, successful or not.
    pub(crate) fn logins(&self) -> usize {
        self.counters.logins.load(Ordering::SeqCst)
    }

    /// How many `LIST` commands arrived — what says whether the folder
    /// snapshot was reused or re-fetched.
    pub(crate) fn lists(&self) -> usize {
        self.counters.lists.load(Ordering::SeqCst)
    }

    /// How many `UID FETCH` commands arrived — what says whether a body was
    /// read once or once per keystroke.
    pub(crate) fn fetches(&self) -> usize {
        self.counters.fetches.load(Ordering::SeqCst)
    }

    /// The messages the client filed, in the order they arrived.
    pub(crate) fn appended(&self) -> Vec<Appended> {
        self.counters.appended.lock().expect("not poisoned").clone()
    }

    /// The `UID STORE` command lines the client sent, verbatim.
    pub(crate) fn stored(&self) -> Vec<String> {
        self.counters.stored.lock().expect("not poisoned").clone()
    }
}

async fn serve(sock: tokio::net::TcpStream, script: Script, nth: usize, counters: Arc<Counters>) {
    if nth < script.close_first {
        // Dropped before the greeting: the client sees the connection go,
        // which is a transport failure and nothing it can ask the user about.
        return;
    }
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
    // What `SELECT` put us in — the fake server has to keep it for the same
    // reason a real one does: `UID SEARCH` and `UID FETCH` mean nothing
    // without it.
    let mut selected: Option<&FakeFolder> = None;
    while let Ok(Some(line)) = lines.next_line().await {
        commands += 1;
        if nth == 0 && script.hang_up_after == Some(commands) {
            // Gone without a word — exactly what a timed-out session looks
            // like from the client side.
            return;
        }
        if nth == 0 && script.stall_at == Some(commands) {
            // Still there, still silent. Nothing but a deadline gets the
            // client out of this.
            tokio::time::sleep(Duration::from_secs(3600)).await;
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
                counters.logins.fetch_add(1, Ordering::SeqCst);
                if args.get(1).map(String::as_str) == Some(script.password) {
                    format!("{tag} OK LOGIN completed\r\n")
                } else {
                    format!("{tag} NO [AUTHENTICATIONFAILED] Invalid credentials\r\n")
                }
            }
            "LIST" => {
                counters.lists.fetch_add(1, Ordering::SeqCst);
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
            "SELECT" | "EXAMINE" => {
                match quoted(0).and_then(|name| script.folders.iter().find(|f| f.path == name)) {
                    Some(folder) => {
                        selected = Some(folder);
                        format!(
                            "* {} EXISTS\r\n* 0 RECENT\r\n* OK [UIDVALIDITY {}] UIDs valid\r\n\
                             * FLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft)\r\n\
                             {tag} OK [READ-WRITE] SELECT completed\r\n",
                            folder.mails.len(),
                            folder.uid_validity
                        )
                    }
                    None => {
                        selected = None;
                        format!("{tag} NO [NONEXISTENT] no such mailbox\r\n")
                    }
                }
            }
            // `UID SEARCH …` / `UID FETCH …` — the subcommand is the first
            // argument, which is why UID commands cannot be matched by verb
            // alone.
            "UID" => {
                let sub = args
                    .first()
                    .map(|s| s.to_ascii_uppercase())
                    .unwrap_or_default();
                match (selected, sub.as_str()) {
                    (None, _) => format!("{tag} BAD no mailbox selected\r\n"),
                    (Some(folder), "SEARCH") => {
                        let uids: Vec<String> =
                            folder.mails.iter().map(|m| m.uid.to_string()).collect();
                        format!(
                            "* SEARCH {}\r\n{tag} OK SEARCH completed\r\n",
                            uids.join(" ")
                        )
                    }
                    (Some(folder), "FETCH") => {
                        counters.fetches.fetch_add(1, Ordering::SeqCst);
                        let wanted: Vec<u32> = args
                            .get(1)
                            .map(|set| {
                                set.split(',')
                                    .filter_map(|u| u.parse::<u32>().ok())
                                    .collect()
                            })
                            .unwrap_or_default();
                        // Which sections the client asked for. A real server
                        // answers what was requested and nothing else, and
                        // that is exactly what the body path depends on.
                        let sections = requested_sections(&line);
                        let mut out = String::new();
                        for (seq, mail) in folder.mails.iter().enumerate() {
                            if !wanted.contains(&mail.uid) {
                                continue;
                            }
                            out.push_str(&match sections.as_slice() {
                                [] => fetch_line(seq + 1, mail),
                                wanted => body_line(seq + 1, mail, wanted),
                            });
                        }
                        out.push_str(&format!("{tag} OK FETCH completed\r\n"));
                        out
                    }
                    (Some(_), "STORE") => {
                        counters
                            .stored
                            .lock()
                            .expect("not poisoned")
                            .push(line.clone());
                        // `.SILENT` is what the client asks for, so there is
                        // nothing untagged to send back.
                        format!("{tag} OK STORE completed\r\n")
                    }
                    (_, other) => format!("{tag} BAD unknown UID subcommand {other}\r\n"),
                }
            }
            // The one command that is not a line: the client announces a
            // byte count, waits for a continuation, and then writes the
            // message. Reading it back line by line is exact as long as the
            // literal ends where it says it does — which is the client's
            // side of the same contract.
            "APPEND" => {
                let folder = quoted(0).unwrap_or_default();
                let flags = line
                    .split_once('(')
                    .and_then(|(_, rest)| rest.split_once(')'))
                    .map(|(flags, _)| flags.to_string())
                    .unwrap_or_default();
                let size: usize = line
                    .rsplit_once('{')
                    .and_then(|(_, rest)| rest.trim_end_matches('}').parse().ok())
                    .unwrap_or(0);
                if tx.write_all(b"+ ready for the literal\r\n").await.is_err() {
                    return;
                }
                let mut source = String::new();
                while source.len() < size {
                    let Ok(Some(part)) = lines.next_line().await else {
                        return;
                    };
                    source.push_str(&part);
                    source.push_str("\r\n");
                }
                // The CRLF that closes the literal, and is not part of it.
                let _ = lines.next_line().await;
                counters
                    .appended
                    .lock()
                    .expect("not poisoned")
                    .push(Appended {
                        folder,
                        flags,
                        source,
                    });
                format!("{tag} OK [APPENDUID 45 9] APPEND completed\r\n")
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

/// One `* n FETCH (…)` line, in the shape a real server sends it.
///
/// Written out by hand on purpose: the point of the fixture is that the
/// client's parser meets the real grammar — `ENVELOPE` with its ten
/// positional fields, addresses as four-element lists, and a `BODYSTRUCTURE`
/// that is a nested list for a multipart message and a flat one otherwise.
fn fetch_line(seq: usize, mail: &FakeMessage) -> String {
    let quoted_or_nil = |v: &str| {
        if v.is_empty() {
            "NIL".to_string()
        } else {
            format!("\"{v}\"")
        }
    };
    let from = format!(
        "(({} NIL \"{}\" \"{}\"))",
        quoted_or_nil(mail.from_name),
        mail.from_mailbox,
        mail.from_host
    );
    let envelope = format!(
        "(\"{}\" {} {from} {from} NIL ((NIL NIL \"me\" \"example.org\")) NIL NIL NIL \"<{}@example.org>\")",
        mail.date,
        quoted_or_nil(mail.subject),
        mail.uid
    );
    let text_part = "(\"TEXT\" \"PLAIN\" (\"CHARSET\" \"UTF-8\") NIL NIL \"7BIT\" 120 4)";
    let pdf_part = concat!(
        "(\"APPLICATION\" \"PDF\" (\"NAME\" \"invoice.pdf\") NIL NIL \"BASE64\" 4096 NIL ",
        "(\"ATTACHMENT\" (\"FILENAME\" \"invoice.pdf\")) NIL)"
    );
    let body = if mail.attachment {
        format!("({text_part}{pdf_part} \"MIXED\")")
    } else {
        text_part.to_string()
    };
    format!(
        concat!(
            "* {seq} FETCH (UID {uid} FLAGS ({flags}) ",
            "INTERNALDATE \"01-Sep-2026 09:00:00 +0200\" RFC822.SIZE {size} ",
            "ENVELOPE {envelope} BODYSTRUCTURE {body})\r\n"
        ),
        seq = seq,
        uid = mail.uid,
        flags = mail.flags,
        size = mail.size,
        envelope = envelope,
        body = body
    )
}

/// The `BODY[…]` sections a `FETCH` line asks for, in order. `BODY[]` — the
/// whole message — comes back as an empty string, which is also how it is
/// written on the wire.
fn requested_sections(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(open) = rest.find("BODY") {
        rest = &rest[open + "BODY".len()..];
        // `BODY.PEEK[…]` and `BODY[…]` name the same section; only the
        // `\Seen` side effect differs, and this server sets no flags.
        let rest_trimmed = rest.strip_prefix(".PEEK").unwrap_or(rest);
        let Some(inner) = rest_trimmed.strip_prefix('[') else {
            continue;
        };
        let Some(close) = inner.find(']') else {
            break;
        };
        out.push(inner[..close].to_string());
        rest = &inner[close..];
    }
    out
}

/// A `FETCH` response carrying literals — the body, or named MIME sections.
///
/// Literals rather than quoted strings because that is what a real server
/// sends for anything that may contain a newline, and the byte count in the
/// braces is the whole point: a client that miscounts desynchronises the
/// stream instead of returning a wrong value.
fn body_line(seq: usize, mail: &FakeMessage, sections: &[String]) -> String {
    let mut out = format!("* {seq} FETCH (UID {}", mail.uid);
    for section in sections {
        let payload: Option<&str> = if section.is_empty() {
            Some(mail.source)
        } else if let Some(part) = section.strip_suffix(".MIME") {
            mail.parts
                .iter()
                .find(|(p, _, _)| *p == part)
                .map(|(_, h, _)| *h)
        } else {
            mail.parts
                .iter()
                .find(|(p, _, _)| p == section)
                .map(|(_, _, b)| *b)
        };
        // A section the message does not have is simply absent from the
        // response — the client must not read that as an empty file.
        if let Some(payload) = payload {
            out.push_str(&format!(
                " BODY[{section}] {{{}}}\r\n{payload}",
                payload.len()
            ));
        }
    }
    out.push_str(")\r\n");
    out
}
