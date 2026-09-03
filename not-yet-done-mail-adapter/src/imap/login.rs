//! Getting from a hostname to a logged-in session.
//!
//! Two steps, kept apart because they fail differently: [`connect`] is
//! network and TLS, [`login`] is credentials. Only the second one is worth
//! asking the user about, and only the second one may invalidate what the
//! password store handed us.
//!
//! `async-imap` ships no STARTTLS helper, so [`connect`] performs the upgrade
//! itself: greeting, `STARTTLS`, unwrap the stream, wrap it in TLS, and start
//! a fresh client over it — deliberately *without* reading a greeting the
//! second time, because RFC 2595 says the server sends none after the
//! handshake.

use std::collections::HashMap;
use std::time::Duration;

use async_imap::{Authenticator, Client};
use async_native_tls::TlsConnector;
use not_yet_done_content::StatusReporter;
use tokio::net::TcpStream;
use tokio::time::timeout;

use super::stream::MailStream;
use crate::config::{AccountConfig, Security};
use crate::error::{MailError, MailResult, classify};

/// How long the TCP connect and the TLS handshake each get. A mail server
/// that has not answered in half a minute is down, not slow.
pub(crate) const CONNECT_TIMEOUT_SECS: u64 = 30;
/// How long the login exchange gets. Generous: a credential script may be
/// waiting on a hardware token behind it.
pub(crate) const LOGIN_TIMEOUT_SECS: u64 = 120;

/// A client that has completed the greeting (and, for `starttls`, the
/// upgrade) but has not logged in.
pub(crate) type MailClient = Client<MailStream>;
pub(crate) type MailSession = async_imap::Session<MailStream>;

/// Open the transport and read the server greeting.
pub(crate) async fn connect(
    account: &AccountConfig,
    status: &StatusReporter,
    who: &str,
) -> MailResult<MailClient> {
    let addr = (account.host.as_str(), account.port());
    status.connect_phase(
        format!("{who}: connecting to {}:{}", addr.0, addr.1),
        0,
        CONNECT_TIMEOUT_SECS,
    );
    let tcp = with_timeout(
        CONNECT_TIMEOUT_SECS,
        TcpStream::connect(addr),
        &format!("connecting to {}:{}", addr.0, addr.1),
    )
    .await?
    .map_err(|e| MailError::Transport(format!("cannot reach {}:{}: {e}", addr.0, addr.1)))?;
    // Nagle would sit on the small command lines IMAP is made of.
    let _ = tcp.set_nodelay(true);

    let stream = match account.security {
        Security::Tls => {
            status.connect_step(format!("{who}: TLS handshake"));
            MailStream::Tls(Box::new(tls_wrap(account, tcp).await?))
        }
        Security::None => MailStream::Plain(tcp),
        Security::Starttls => {
            // The greeting has to be consumed before the upgrade, on the
            // plain stream — it is already on the wire when we connect.
            let mut plain = Client::new(MailStream::Plain(tcp));
            greeting(&mut plain, who).await?;
            status.connect_step(format!("{who}: STARTTLS"));
            with_timeout(
                CONNECT_TIMEOUT_SECS,
                plain.run_command_and_check_ok("STARTTLS", None),
                "STARTTLS",
            )
            .await?
            .map_err(|e| match classify(e) {
                // A server that refuses the upgrade is a configuration
                // problem, not a flaky network: say so, rather than letting
                // it read as a lost connection.
                MailError::Server(msg) => MailError::Config(format!(
                    "{} does not offer STARTTLS ({msg}) — try `security: tls` on port 993",
                    account.host
                )),
                other => other,
            })?;
            let tcp = match plain.into_inner() {
                MailStream::Plain(tcp) => tcp,
                // Unreachable: this arm built the plain variant three lines
                // up. Stated as an error rather than a panic all the same.
                MailStream::Tls(_) => {
                    return Err(MailError::Transport("stream was already encrypted".into()));
                }
            };
            status.connect_step(format!("{who}: TLS handshake"));
            return Ok(Client::new(MailStream::Tls(Box::new(
                tls_wrap(account, tcp).await?,
            ))));
            // No second greeting: RFC 2595 §3.1 — the server does not repeat
            // it after the handshake, and waiting for one would hang here.
        }
    };

    let mut client = Client::new(stream);
    greeting(&mut client, who).await?;
    Ok(client)
}

/// Authenticate an open client. The mechanism comes from the account's
/// `auth:` block and was validated against [`crate::auth::MECHANISMS`] when
/// the config was read, so an unknown one here is a bug, not user input.
pub(crate) async fn login(
    client: MailClient,
    mechanism: &str,
    creds: &HashMap<String, String>,
    status: &StatusReporter,
    who: &str,
) -> MailResult<MailSession> {
    let username = field(creds, "username")?;
    status.connect_phase(
        format!("{who}: logging in as {username}"),
        0,
        LOGIN_TIMEOUT_SECS,
    );

    let attempt = match mechanism {
        "password" => {
            let password = field(creds, "password")?;
            let fut = client.login(username.clone(), password);
            with_timeout(LOGIN_TIMEOUT_SECS, fut, "login").await?
        }
        "xoauth2" => {
            let token = field(creds, "token")?;
            let fut = client.authenticate(
                "XOAUTH2",
                XOAuth2 {
                    user: username.clone(),
                    token,
                },
            );
            with_timeout(LOGIN_TIMEOUT_SECS, fut, "login").await?
        }
        other => {
            return Err(MailError::Config(format!(
                "unsupported auth mechanism `{other}`"
            )));
        }
    };

    match attempt {
        Ok(session) => Ok(session),
        // Whatever the server's phrasing, a refused login is an auth error:
        // it is the one conversational `NO` that must cost the session and
        // the cached credentials.
        Err((e, _client)) => Err(MailError::Auth(match classify(e) {
            MailError::Server(msg) | MailError::Transport(msg) | MailError::Parse(msg) => msg,
            other => other.to_string(),
        })),
    }
}

/// SASL XOAUTH2: one client-first message, and — on failure — an empty
/// response to the server's error challenge so the exchange terminates
/// cleanly instead of leaving the connection mid-command.
struct XOAuth2 {
    user: String,
    token: String,
}

impl Authenticator for XOAuth2 {
    type Response = Vec<u8>;

    fn process(&mut self, _challenge: &[u8]) -> Self::Response {
        if self.token.is_empty() {
            return Vec::new();
        }
        let msg = format!("user={}\x01auth=Bearer {}\x01\x01", self.user, self.token);
        // Sent once. Emptying the token makes any further challenge — which
        // is how the server reports a rejected token — answer with the empty
        // line the protocol asks for.
        self.token.clear();
        msg.into_bytes()
    }
}

async fn tls_wrap(
    account: &AccountConfig,
    tcp: TcpStream,
) -> MailResult<async_native_tls::TlsStream<TcpStream>> {
    let connector = TlsConnector::new().danger_accept_invalid_certs(account.accept_invalid_certs);
    with_timeout(
        CONNECT_TIMEOUT_SECS,
        connector.connect(account.host.as_str(), tcp),
        "TLS handshake",
    )
    .await?
    .map_err(|e| MailError::Transport(format!("TLS handshake with {} failed: {e}", account.host)))
}

/// Read the untagged `* OK` the server opens with. Doing it explicitly (and
/// with a deadline) turns a server that accepts the connection and then says
/// nothing into a clear error instead of a hang.
async fn greeting(client: &mut MailClient, who: &str) -> MailResult<()> {
    let res = with_timeout(
        CONNECT_TIMEOUT_SECS,
        client.read_response(),
        "server greeting",
    )
    .await?;
    match res {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(MailError::Transport(format!(
            "{who}: the server closed the connection without a greeting"
        ))),
        Err(e) => Err(MailError::Transport(format!("{who}: no greeting: {e}"))),
    }
}

fn field(creds: &HashMap<String, String>, name: &str) -> MailResult<String> {
    match creds.get(name).map(|v| v.trim()) {
        Some(v) if !v.is_empty() => Ok(v.to_string()),
        _ => Err(MailError::Auth(format!(
            "the `{name}` credential is empty — check the account's `auth:` bindings"
        ))),
    }
}

async fn with_timeout<F: Future>(secs: u64, fut: F, what: &str) -> MailResult<F::Output> {
    timeout(Duration::from_secs(secs), fut)
        .await
        .map_err(|_| MailError::Transport(format!("{what} timed out after {secs}s")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SASL message is exact: `user=…^Aauth=Bearer …^A^A`. A stray space
    /// or a missing control byte is rejected by the server with a challenge
    /// that reads like a credential error, so it is worth pinning.
    #[test]
    fn the_xoauth2_message_has_the_shape_the_rfc_asks_for() {
        let mut auth = XOAuth2 {
            user: "someone@example.invalid".into(),
            token: "tok".into(),
        };
        let first = auth.process(b"");
        assert_eq!(
            String::from_utf8(first).unwrap(),
            "user=someone@example.invalid\u{1}auth=Bearer tok\u{1}\u{1}"
        );
        // The server answers a bad token with an error challenge; the reply
        // to that is an empty line, not the credentials again.
        assert!(auth.process(b"{\"status\":\"401\"}").is_empty());
    }

    #[test]
    fn a_missing_credential_names_the_field_it_wants() {
        let mut creds = HashMap::new();
        creds.insert("username".to_string(), "  ".to_string());
        let err = field(&creds, "username").expect_err("blank is missing");
        assert!(err.to_string().contains("username"), "{err}");
        assert!(err.is_auth(), "a missing credential is an auth problem");
    }
}
