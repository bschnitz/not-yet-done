//! The IMAP commands themselves — plain async functions over a live session.
//!
//! Everything here takes `&mut MailSession` and returns
//! [`crate::model`] shapes, so the connection actor stays a scheduler and
//! nothing above this line ever sees an `async_imap` type. Adding a command
//! is a function here plus one arm in the actor's match.

use async_imap::error::Error as ImapError;
use async_imap::types::{Capability, NameAttribute};
use futures::StreamExt;

use super::login::MailSession;
use crate::error::{MailError, MailResult, classify};
use crate::model::FolderInfo;

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

/// Every mailbox the account has, in the server's own spelling.
///
/// `LIST "" "*"` rather than `LSUB`: subscriptions are a per-client notion,
/// and a folder Thunderbird never subscribed to would otherwise be invisible
/// here for no reason the user could see.
pub(crate) async fn list_folders(session: &mut MailSession) -> MailResult<Vec<FolderInfo>> {
    let mut out = Vec::new();
    {
        // The pattern is interpolated unquoted, so the quotes are ours.
        let names = session
            .list(Some(""), Some("\"*\""))
            .await
            .map_err(classify)?;
        futures::pin_mut!(names);
        while let Some(name) = names.next().await {
            let name = name.map_err(classify)?;
            let selectable = !name.attributes().contains(&NameAttribute::NoSelect);
            out.push(FolderInfo::from_wire(
                name.name(),
                name.delimiter(),
                selectable,
            ));
        }
    }
    drain_unsolicited(session);
    // The same trap as in `capabilities`: a stream that simply ends reads as a
    // complete, empty answer. Every account has an INBOX, so nothing at all
    // means the session died mid-command — and saying so earns the reconnect.
    if out.is_empty() {
        return Err(MailError::Transport(
            "the server ended the connection while listing folders".into(),
        ));
    }
    crate::model::sort_for_display(&mut out);
    Ok(out)
}

/// Message and unread counts for one mailbox, without selecting it.
///
/// Worth knowing: in a `STATUS` response `unseen` is the **count** of unseen
/// messages, while in a `SELECT` response the same field is the sequence
/// number of the first unseen one. Reading the second as the first is how a
/// folder ends up claiming 1 unread mail forever.
pub(crate) async fn folder_status(session: &mut MailSession, path: &str) -> MailResult<(u32, u32)> {
    let mbox = session
        .status(path, "(MESSAGES UNSEEN)")
        .await
        .map_err(classify)?;
    drain_unsolicited(session);
    // We asked for UNSEEN by name, so an answer without it is not an answer.
    let Some(unseen) = mbox.unseen else {
        return Err(MailError::Transport(format!(
            "the server ended the connection while reporting on `{path}`"
        )));
    };
    Ok((mbox.exists, unseen))
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
