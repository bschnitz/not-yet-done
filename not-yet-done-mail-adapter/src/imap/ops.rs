//! The IMAP commands themselves — plain async functions over a live session.
//!
//! Everything here takes `&mut SessionState` and returns
//! [`crate::model`] shapes, so the connection actor stays a scheduler and
//! nothing above this line ever sees an `async_imap` type. Adding a command
//! is a function here plus one arm in the actor's match.

use async_imap::error::Error as ImapError;
use async_imap::imap_proto::types::{MessageSection, SectionPath};
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

/// The mailbox the server marks `\Sent` (RFC 6154 SPECIAL-USE), if it marks
/// one.
///
/// A second `LIST` rather than a field on [`FolderInfo`]: the answer is
/// wanted once per sent message and never while a folder tree is on screen,
/// and the tree pays for every byte it carries per row.
///
/// The stream is read to its end even after a hit — abandoning a response
/// mid-flight leaves the rest of it on the wire for the next command to
/// stumble over.
pub(crate) async fn sent_folder(state: &mut SessionState) -> MailResult<Option<String>> {
    let session = &mut state.session;
    let mut found: Option<String> = None;
    {
        let names = session
            .list(Some(""), Some("\"*\""))
            .await
            .map_err(classify)?;
        futures::pin_mut!(names);
        while let Some(name) = names.next().await {
            let name = name.map_err(classify)?;
            if found.is_none() && name.attributes().contains(&NameAttribute::Sent) {
                found = Some(name.name().to_string());
            }
        }
    }
    drain_unsolicited(session);
    Ok(found)
}

/// Put a message into a mailbox — the copy of an outgoing mail that makes it
/// appear in Sent.
///
/// SMTP hands a message to a server and forgets it; nothing shows up in Sent
/// unless the client puts it there. `flags` is an IMAP flag list including
/// its parentheses, `(\Seen)` for something the sender wrote themselves.
pub(crate) async fn append(
    state: &mut SessionState,
    path: &str,
    flags: &str,
    source: &[u8],
) -> MailResult<()> {
    let session = &mut state.session;
    session
        .append(path, Some(flags), None, source)
        .await
        .map_err(classify)?;
    drain_unsolicited(session);
    Ok(())
}

/// Set or clear flags on a set of messages — `\Answered` after a reply,
/// `\Seen` and `\Flagged` from the message level's own actions.
///
/// `.SILENT`: the updated flags are not wanted back. The pane reloads after
/// the action either way, and an unsolicited `FETCH` here would only be
/// drained.
///
/// The set is a slice rather than one uid because the command takes one:
/// marking a selection read is a single round trip, and building that out of
/// N calls would be N `SELECT`-checked round trips for no reason.
pub(crate) async fn store_flags(
    state: &mut SessionState,
    path: &str,
    uid_validity: u32,
    uids: &[u32],
    add: bool,
    flags: &str,
) -> MailResult<()> {
    if uids.is_empty() {
        return Ok(());
    }
    select_stable(state, path, uid_validity).await?;
    let sign = if add { '+' } else { '-' };
    {
        let updates = state
            .session
            .uid_store(uid_set(uids), format!("{sign}FLAGS.SILENT ({flags})"))
            .await
            .map_err(classify)?;
        futures::pin_mut!(updates);
        while let Some(update) = updates.next().await {
            update.map_err(classify)?;
        }
    }
    drain_unsolicited(&mut state.session);
    Ok(())
}

/// Move messages to another mailbox.
///
/// Two ways there, and which one is available is the server's to say:
///
/// * `MOVE` (RFC 6851) does it in one command, and atomically per message —
///   no message is ever in both mailboxes or in neither.
/// * Without it, the move is `COPY` + `\Deleted` + expunge. That last step
///   is the dangerous one: a plain `EXPUNGE` removes **every** message in
///   the mailbox that carries `\Deleted`, including ones another client
///   marked and has not expunged yet. `UID EXPUNGE` (RFC 4315, `UIDPLUS`)
///   removes only ours.
///
/// A server offering neither is refused rather than served with a plain
/// `EXPUNGE`: silently deleting mail nobody asked about is worse than a move
/// that does not happen and says why.
pub(crate) async fn move_messages(
    state: &mut SessionState,
    path: &str,
    uid_validity: u32,
    uids: &[u32],
    dest: &str,
) -> MailResult<()> {
    if uids.is_empty() {
        return Ok(());
    }
    select_stable(state, path, uid_validity).await?;
    let set = uid_set(uids);
    if has_capability(state, "MOVE").await? {
        state.session.uid_mv(&set, dest).await.map_err(classify)?;
        drain_unsolicited(&mut state.session);
        return Ok(());
    }
    if !has_capability(state, "UIDPLUS").await? {
        return Err(MailError::Server(format!(
            "this server offers neither MOVE nor UIDPLUS, so a message copied to `{dest}` \
             could only be removed from `{path}` by an EXPUNGE that would also delete \
             anything else marked deleted there"
        )));
    }
    state.session.uid_copy(&set, dest).await.map_err(classify)?;
    // Only now: a copy that failed must leave the original alone.
    store_flags(state, path, uid_validity, uids, true, "\\Deleted").await?;
    {
        let expunged = state.session.uid_expunge(&set).await.map_err(classify)?;
        futures::pin_mut!(expunged);
        while let Some(uid) = expunged.next().await {
            uid.map_err(classify)?;
        }
    }
    drain_unsolicited(&mut state.session);
    Ok(())
}

/// Whether the server named `name` in its capability list, asking at most
/// once per session (see [`SessionState::cached_caps`]).
async fn has_capability(state: &mut SessionState, name: &str) -> MailResult<bool> {
    if state.cached_caps().is_none() {
        let caps = capabilities(state).await?;
        state.remember_caps(caps);
    }
    Ok(state
        .cached_caps()
        .unwrap_or_default()
        .iter()
        .any(|c| c.eq_ignore_ascii_case(name)))
}

/// A uid set in the form the command wants it: `4`, or `4,7,9`.
fn uid_set(uids: &[u32]) -> String {
    uids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
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
        let rows = fetch_envelopes(state, &window, selection.uid_validity).await?;
        // The same empty-but-`Ok` trap as in `search`: a stream that ends
        // parses as a complete, empty answer. Here the premise that makes it
        // a contradiction is right above — the search just named these UIDs,
        // so none of them coming back is a dead session, not an empty page.
        if rows.is_empty() {
            return Err(MailError::Transport(format!(
                "the server ended the connection while fetching envelopes from `{path}`"
            )));
        }
        rows
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
    let found = state.session.uid_search(query).await.map_err(classify)?;
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
    // Emptiness is *not* judged here: whether no rows means a dead session or
    // simply a message that is gone depends on what the caller asked, and
    // only the caller knows.
    let position = |uid: u32| window.iter().position(|w| *w == uid).unwrap_or(usize::MAX);
    rows.sort_by_key(|r| position(r.uid));
    Ok(rows)
}

/// Select `path` and insist it is still the mailbox the caller's ids were
/// minted in.
///
/// This is what the UIDVALIDITY in a message id is *for* (see
/// [`crate::ids`]). A server that renumbers a mailbox bumps the value, and
/// every UID minted before that now points at a different message — so a
/// bookmark from yesterday must fail loudly here rather than open the wrong
/// mail. The session is fine either way, which is why this is a `Server`
/// error and not a `Transport` one.
async fn select_stable(state: &mut SessionState, path: &str, uid_validity: u32) -> MailResult<()> {
    let selection = state.select(path).await?;
    if selection.uid_validity != uid_validity {
        return Err(MailError::Server(format!(
            "`{path}` was renumbered by the server (UIDVALIDITY {uid_validity} → {}); \
             reload the folder to address its messages again",
            selection.uid_validity
        )));
    }
    Ok(())
}

/// One message, whole, as it travelled.
///
/// `BODY.PEEK[]` rather than `BODY[]`: reading a message in a preview pane
/// must not mark it `\Seen` behind the user's back — that is a decision the
/// unread wiring makes deliberately (phase 5), not a side effect of moving
/// the cursor.
///
/// The whole source is fetched, attachments included, because that is what a
/// body costs: the text part cannot be decoded without its own MIME headers,
/// and a mail client that opens a message downloads the message. What it buys
/// is that no *further* round trip is needed to render it.
pub(crate) async fn message_source(
    state: &mut SessionState,
    path: &str,
    uid_validity: u32,
    uid: u32,
) -> MailResult<Vec<u8>> {
    select_stable(state, path, uid_validity).await?;
    let mut source = None;
    {
        let fetches = state
            .session
            .uid_fetch(uid.to_string(), "(BODY.PEEK[])")
            .await
            .map_err(classify)?;
        futures::pin_mut!(fetches);
        while let Some(fetch) = fetches.next().await {
            let fetch = fetch.map_err(classify)?;
            if let Some(body) = fetch.body() {
                source = Some(body.to_vec());
            }
        }
    }
    drain_unsolicited(&mut state.session);
    source.ok_or_else(|| {
        MailError::Transport(format!(
            "the server returned no body for message {uid} in `{path}`"
        ))
    })
}

/// The decoded bytes of one MIME part — an attachment, as a file.
///
/// Two sections are asked for in the one command: the part's own MIME
/// headers and its body. The headers are not decoration — they carry the
/// transfer encoding, without which base64 would be written to disk as
/// base64. Fetching only the part (and not the whole message) is what keeps
/// opening a 30 kB PDF out of a 20 MB mail cheap.
pub(crate) async fn message_part(
    state: &mut SessionState,
    path: &str,
    uid_validity: u32,
    uid: u32,
    part: &str,
) -> MailResult<Vec<u8>> {
    let section: Vec<u32> = part
        .split('.')
        .map(|n| n.parse::<u32>())
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| MailError::Parse(format!("`{part}` is not a MIME part path")))?;
    if section.is_empty() {
        return Err(MailError::Parse("empty MIME part path".into()));
    }
    select_stable(state, path, uid_validity).await?;

    let items = format!("(BODY.PEEK[{part}.MIME] BODY.PEEK[{part}])");
    let mut found = None;
    {
        let fetches = state
            .session
            .uid_fetch(uid.to_string(), &items)
            .await
            .map_err(classify)?;
        futures::pin_mut!(fetches);
        while let Some(fetch) = fetches.next().await {
            let fetch = fetch.map_err(classify)?;
            let headers = fetch
                .section(&SectionPath::Part(
                    section.clone(),
                    Some(MessageSection::Mime),
                ))
                .unwrap_or_default();
            let Some(body) = fetch.section(&SectionPath::Part(section.clone(), None)) else {
                continue;
            };
            found = Some(crate::mime::decode_part(headers, body));
        }
    }
    drain_unsolicited(&mut state.session);
    found.ok_or_else(|| {
        MailError::Server(format!(
            "the server returned no part `{part}` of message {uid} in `{path}`"
        ))
    })
}

/// The envelope of a single message — the same row a listing produces, for
/// one UID.
///
/// Needed where a message is reached without its list: a restored cursor, or
/// an attachment level opened straight from a bookmark. One round trip, the
/// cheap attribute set, no body.
pub(crate) async fn message_envelope(
    state: &mut SessionState,
    path: &str,
    uid_validity: u32,
    uid: u32,
) -> MailResult<EnvelopeRow> {
    select_stable(state, path, uid_validity).await?;
    let mut rows = fetch_envelopes(state, &[uid], uid_validity).await?;
    if rows.is_empty() {
        return Err(MailError::Server(format!(
            "no message {uid} in `{path}` — it may have been moved or deleted"
        )));
    }
    Ok(rows.remove(0))
}
