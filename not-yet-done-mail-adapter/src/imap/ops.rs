//! The IMAP commands themselves — plain async functions over a live session.
//!
//! Everything here takes `&mut SessionState` and returns
//! [`crate::model`] shapes, so the connection actor stays a scheduler and
//! nothing above this line ever sees an `async_imap` type. Adding a command
//! is a function here plus one arm in the actor's match.

use async_imap::error::Error as ImapError;
use async_imap::types::{Capability, NameAttribute};
use futures::StreamExt;

use super::envelope;
use super::login::MailSession;
use super::session::SessionState;
use crate::error::{MailError, MailResult, classify};
use crate::model::{EnvelopeRow, FolderInfo};

/// What the server says it can do. Also the liveness check the actor uses
/// when a view asks for a connection and nothing else: it is one round trip,
/// and an idle-timed-out session fails it rather than failing the user's
/// actual request.
pub(crate) async fn capabilities(state: &mut SessionState) -> MailResult<Vec<String>> {
    let session = &mut state.session;
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
pub(crate) async fn list_folders(state: &mut SessionState) -> MailResult<Vec<FolderInfo>> {
    let session = &mut state.session;
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
pub(crate) async fn folder_status(state: &mut SessionState, path: &str) -> MailResult<(u32, u32)> {
    let session = &mut state.session;
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
pub(crate) async fn logout(state: &mut SessionState) {
    let _: Result<(), ImapError> = state.session.logout().await;
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

/// One page of a folder's messages, plus what it took to know the page
/// exists.
#[derive(Debug)]
pub(crate) struct MessagePage {
    pub(crate) rows: Vec<EnvelopeRow>,
    /// How many messages the search matched in total — the number the pager
    /// shows, and the reason the search is issued even for page 1.
    pub(crate) total: u32,
    pub(crate) uid_validity: u32,
}

/// The attributes one row needs. Deliberately the cheap set: no body, no
/// header text, so a page of fifty costs fifty envelopes and not fifty mails.
const ENVELOPE_ITEMS: &str = "(UID FLAGS INTERNALDATE RFC822.SIZE ENVELOPE BODYSTRUCTURE)";

/// One page of `path`, matching `query` (IMAP SEARCH; empty means `ALL`).
///
/// The shape is what keeps a 100k folder affordable: `UID SEARCH` returns the
/// matching ids — a few hundred kilobytes at worst — and only the window the
/// pager asked for is fetched. Nothing here reads a message body.
pub(crate) async fn messages(
    state: &mut SessionState,
    path: &str,
    query: &str,
    offset: u32,
    limit: u32,
) -> MailResult<MessagePage> {
    let selection = state.select(path).await?;
    let uids = search(state, query, &selection).await?;
    let total = uids.len() as u32;
    let window: Vec<u32> = uids
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect();
    let rows = if window.is_empty() {
        Vec::new()
    } else {
        fetch_envelopes(state, &window, selection.uid_validity).await?
    };
    Ok(MessagePage {
        rows,
        total,
        uid_validity: selection.uid_validity,
    })
}

/// The matching UIDs, newest first.
///
/// Descending UID is the default order because it is the one IMAP gives for
/// free: UIDs ascend with arrival, so reversing them is "newest first" without
/// a `SORT` round trip and without reading a single date.
async fn search(
    state: &mut SessionState,
    query: &str,
    selection: &super::session::Selection,
) -> MailResult<Vec<u32>> {
    let query = query.trim();
    let query = if query.is_empty() { "ALL" } else { query };
    let found = state
        .session
        .uid_search(query)
        .await
        .map_err(classify)?;
    drain_unsolicited(&mut state.session);
    let mut uids: Vec<u32> = found.into_iter().collect();
    uids.sort_unstable_by(|a, b| b.cmp(a));
    // The same empty-but-`Ok` trap as in `capabilities`: a stream that ends
    // parses as a complete, empty answer. An empty result is perfectly
    // legitimate here — an empty folder, a search that matched nothing — so
    // the guard is narrow: only `ALL` over a mailbox `SELECT` just said holds
    // messages is a contradiction, and that one is the dead session.
    if uids.is_empty() && query == "ALL" && selection.exists > 0 {
        return Err(MailError::Transport(format!(
            "the server ended the connection while searching `{}`",
            selection.path
        )));
    }
    Ok(uids)
}

/// Envelopes for an explicit set of UIDs, returned in the order asked for.
///
/// The order matters: a server may answer a `UID FETCH` in any order it
/// likes, and the caller has already decided what the page looks like.
async fn fetch_envelopes(
    state: &mut SessionState,
    window: &[u32],
    uid_validity: u32,
) -> MailResult<Vec<EnvelopeRow>> {
    let set = window
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let mut rows = Vec::with_capacity(window.len());
    {
        let fetches = state
            .session
            .uid_fetch(&set, ENVELOPE_ITEMS)
            .await
            .map_err(classify)?;
        futures::pin_mut!(fetches);
        while let Some(fetch) = fetches.next().await {
            let fetch = fetch.map_err(classify)?;
            if let Some(row) = envelope::row_from(&fetch, uid_validity) {
                rows.push(row);
            }
        }
    }
    drain_unsolicited(&mut state.session);
    if rows.is_empty() {
        return Err(MailError::Transport(
            "the server ended the connection while fetching envelopes".into(),
        ));
    }
    let position = |uid: u32| window.iter().position(|w| *w == uid).unwrap_or(usize::MAX);
    rows.sort_by_key(|r| position(r.uid));
    Ok(rows)
}
