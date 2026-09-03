//! The IMAP commands themselves — plain async functions over a live session.
//!
//! Everything here takes `&mut MailSession` and returns
//! [`crate::model`] shapes, so the connection actor stays a scheduler and
//! nothing above this line ever sees an `async_imap` type. Adding a command
//! is a function here plus one arm in the actor's match.

use async_imap::error::Error as ImapError;
use async_imap::types::Capability;

use super::login::MailSession;
use crate::error::{MailError, MailResult, classify};

/// What the server says it can do. Also the liveness check the actor uses
/// when a view asks for a connection and nothing else: it is one round trip,
/// and an idle-timed-out session fails it rather than failing the user's
/// actual request.
pub(crate) async fn capabilities(session: &mut MailSession) -> MailResult<Vec<String>> {
    let caps = session.capabilities().await.map_err(classify)?;
    let mut out: Vec<String> = caps
        .iter()
        .map(|c| match c {
            Capability::Imap4rev1 => "IMAP4rev1".to_string(),
            Capability::Auth(v) => format!("AUTH={v}"),
            Capability::Atom(v) => v.to_string(),
        })
        .collect();
    out.sort();
    drain_unsolicited(session);
    // A trap worth a guard: when the connection dies mid-command,
    // `async-imap` sees the response stream end and returns the capabilities
    // it has collected so far — that is, `Ok(())` with an empty set, not an
    // error. Every server must advertise IMAP4rev1, so an empty answer means
    // the session is gone, and saying so is what earns the reconnect.
    if out.is_empty() {
        return Err(MailError::Transport(
            "the server ended the connection while answering CAPABILITY".into(),
        ));
    }
    Ok(out)
}

/// End the session politely. Failure is not worth reporting — the socket is
/// about to be dropped either way.
pub(crate) async fn logout(session: &mut MailSession) {
    let _: Result<(), ImapError> = session.logout().await;
}

/// Throw away the notifications the server volunteered.
///
/// `async-imap` posts every unsolicited response (`EXISTS`, `EXPUNGE`,
/// `RECENT`, flag updates) into a channel bounded at 100. Nobody reading it
/// means a long-lived session eventually fills it, and a full channel blocks
/// the response reader — the session stops answering. Until phase 6 turns
/// these into cache invalidations, the honest thing is to drop them, and to
/// do it after every command.
pub(crate) fn drain_unsolicited(session: &mut MailSession) {
    while session.unsolicited_responses.try_recv().is_ok() {}
}
