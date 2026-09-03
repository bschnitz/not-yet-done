//! What can go wrong, and — the part that earns the enum — whether the
//! connection survived it.
//!
//! IMAP has no request/response independence: one session, one command at a
//! time, and a mailbox selected on the side. So a failure is either
//! *conversational* (the server understood the command and refused it — the
//! session is still perfectly good) or *fatal* to the session (the socket
//! died, the server timed the connection out, TLS broke). Only the second
//! kind may cost a reconnect; treating the first kind as fatal would
//! reconnect on every `NO [NONEXISTENT]`.

use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub(crate) enum MailError {
    /// The config cannot describe a usable connection. Never retried.
    #[error("{0}")]
    Config(String),
    /// The server rejected the credentials. Fatal to the session, and the
    /// resolved credentials are dropped so the next attempt asks again.
    #[error("login rejected: {0}")]
    Auth(String),
    /// The user closed the credential dialog.
    #[error("login cancelled")]
    Cancelled,
    /// Socket, TLS or protocol framing — the session is gone.
    #[error("{0}")]
    Transport(String),
    /// The server never answered a command. Fatal to the session — a reply
    /// that arrives after we stopped waiting would be read as the answer to
    /// whatever we send next — but never worth repeating: a command that ran
    /// out of time once takes just as long the second time, and the user has
    /// already waited for it.
    #[error("{0}")]
    Timeout(String),
    /// The server answered `NO` or `BAD`. The session is still usable.
    #[error("{0}")]
    Server(String),
    /// A response the adapter could not make sense of.
    #[error("{0}")]
    Parse(String),
    /// The connection actor is gone — the adapter was dropped mid-call.
    #[error("connection closed")]
    Closed,
}

impl MailError {
    /// Whether the session must be thrown away. A fatal error is worth one
    /// reconnect: a mail client that idles for an hour and then loads a
    /// folder meets a server-side timeout as a matter of routine, and the
    /// user should never see that as an error.
    pub(crate) fn is_fatal(&self) -> bool {
        matches!(
            self,
            MailError::Transport(_)
                | MailError::Timeout(_)
                | MailError::Auth(_)
                | MailError::Closed
        )
    }

    /// Whether running the command again on a fresh session is worth it.
    ///
    /// Fatal and worth retrying are not the same question. A session the
    /// server timed out answers the first question yes and the second one
    /// too — that is the routine case the reconnect exists for. A command
    /// that hit *our* deadline answers the first yes and the second no:
    /// nothing about a new session makes a slow server fast, and repeating
    /// it doubles the wait the user is already complaining about.
    pub(crate) fn is_worth_retrying(&self) -> bool {
        self.is_fatal() && !matches!(self, MailError::Timeout(_))
    }

    /// Whether the credentials themselves are suspect, i.e. resolving them
    /// again (and asking, if that is where they come from) is the fix.
    pub(crate) fn is_auth(&self) -> bool {
        matches!(self, MailError::Auth(_))
    }
}

/// Classify an `async_imap` error. `NO`/`BAD` are the server talking back;
/// everything else means the stream is no longer trustworthy.
///
/// A `NO` to the *login* command is the one place where a conversational
/// answer is still fatal — the caller says so by using [`MailError::Auth`]
/// directly; classification cannot know it from the error alone.
pub(crate) fn classify(err: async_imap::error::Error) -> MailError {
    use async_imap::error::Error as E;
    match err {
        E::No(msg) => MailError::Server(format!("server refused: {msg}")),
        E::Bad(msg) => MailError::Server(format!("server rejected the command: {msg}")),
        E::Parse(e) => MailError::Parse(format!("unreadable response: {e}")),
        E::Validate(e) => MailError::Config(format!("invalid argument: {e}")),
        E::Io(e) => MailError::Transport(format!("connection lost: {e}")),
        E::ConnectionLost => MailError::Transport("connection lost".into()),
        other => MailError::Transport(other.to_string()),
    }
}

pub(crate) type MailResult<T> = Result<T, MailError>;

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of the split: a refused command must not cost the
    /// session, or every missing mailbox would trigger a reconnect.
    #[test]
    fn a_refused_command_is_not_fatal_but_a_dead_socket_is() {
        assert!(!classify(async_imap::error::Error::No("NONEXISTENT".into())).is_fatal());
        assert!(!classify(async_imap::error::Error::Bad("syntax".into())).is_fatal());
        assert!(classify(async_imap::error::Error::ConnectionLost).is_fatal());
        assert!(MailError::Auth("bad password".into()).is_fatal());
        assert!(!MailError::Config("no host".into()).is_fatal());
    }

    /// A deadline we set is fatal to the session — a late reply would be
    /// read as the answer to the next command — but repeating it would only
    /// make the user wait twice.
    #[test]
    fn a_timeout_costs_the_session_but_never_a_second_attempt() {
        let timed_out = MailError::Timeout("Loading INBOX 1-50 got no answer in 60s".into());
        assert!(timed_out.is_fatal());
        assert!(!timed_out.is_worth_retrying());
        assert!(MailError::Transport("connection lost".into()).is_worth_retrying());
    }

    #[test]
    fn only_an_auth_error_asks_for_fresh_credentials() {
        assert!(MailError::Auth("nope".into()).is_auth());
        assert!(!MailError::Transport("nope".into()).is_auth());
    }
}
