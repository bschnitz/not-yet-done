//! A live session plus the one piece of state IMAP keeps on the side: which
//! mailbox is currently selected.
//!
//! The selection is cached so that paging through a folder does not pay a
//! `SELECT` per page. What matters is *where* the cache lives: inside the
//! session, not next to it. A session that dies takes its selection with it,
//! so the reconnect that follows cannot inherit a belief about a mailbox it
//! never opened — which is the bug this type exists to make unrepresentable.

use super::login::MailSession;
use crate::error::{MailError, MailResult, classify};

/// What a `SELECT` told us about the mailbox we are now in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Selection {
    pub(crate) path: String,
    /// The mailbox's UIDVALIDITY — half of every message id (see
    /// [`crate::ids`]). A server that renumbers bumps it, and the ids minted
    /// before that stop resolving instead of opening the wrong mail.
    pub(crate) uid_validity: u32,
    /// How many messages the mailbox holds, as `SELECT` reported it.
    pub(crate) exists: u32,
}

pub(crate) struct SessionState {
    pub(crate) session: MailSession,
    selected: Option<Selection>,
    /// What the server said it can do, asked once.
    ///
    /// Here for the same reason the selection is: a capability belongs to a
    /// connection, so a session that dies takes its answer with it. What it
    /// saves is a round trip before every command that has to *choose* a
    /// strategy — `MOVE` against copy-and-expunge is the first one — and
    /// that choice is made on a keypress, where a second round trip is
    /// something the user can feel.
    caps: Option<Vec<String>>,
}

impl SessionState {
    pub(crate) fn new(session: MailSession) -> Self {
        Self {
            session,
            selected: None,
            caps: None,
        }
    }

    /// The remembered capability list, or `None` before anyone asked.
    pub(crate) fn cached_caps(&self) -> Option<&[String]> {
        self.caps.as_deref()
    }

    pub(crate) fn remember_caps(&mut self, caps: Vec<String>) {
        self.caps = Some(caps);
    }

    /// Select `path` unless it is already selected, and report what the
    /// server said about it.
    pub(crate) async fn select(&mut self, path: &str) -> MailResult<Selection> {
        if let Some(current) = self.selected.as_ref().filter(|s| s.path == path) {
            return Ok(current.clone());
        }
        // A failed SELECT leaves *no* mailbox selected on most servers, and
        // the ones that keep the old one are not worth guessing about. Forget
        // first, so a later command cannot run against a mailbox we only
        // think we are in.
        self.selected = None;
        let mbox = self.session.select(path).await.map_err(classify)?;
        super::ops::drain_unsolicited(&mut self.session);
        // Without UIDVALIDITY a message id cannot be minted at all, and one
        // minted from a guess would silently address the wrong message after
        // the next renumber.
        let Some(uid_validity) = mbox.uid_validity else {
            return Err(MailError::Parse(format!(
                "`{path}` was opened without a UIDVALIDITY, so its messages cannot be addressed"
            )));
        };
        let selection = Selection {
            path: path.to_string(),
            uid_validity,
            exists: mbox.exists,
        };
        self.selected = Some(selection.clone());
        Ok(selection)
    }
}
