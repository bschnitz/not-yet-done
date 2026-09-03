//! What the IMAP layer hands upwards: folders, message envelopes, MIME parts.
//!
//! Deliberately protocol-free — no `async_imap` type crosses this line — so
//! the adapter's projection code (and its tests) never needs a live server,
//! and a second backend (JMAP, Maildir) would produce the same shapes.

use chrono::{DateTime, FixedOffset};

/// One mailbox as the folder tree shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FolderInfo {
    /// The mailbox path exactly as the server spells it.
    pub(crate) path: String,
    /// The last path segment — what the tree row is labelled with.
    pub(crate) name: String,
    /// The server's hierarchy delimiter for this mailbox, when it named one.
    pub(crate) delimiter: Option<String>,
    /// `\Noselect` folders exist only to hold children; opening one is an
    /// error, so the row is shown but carries no counts and no messages.
    pub(crate) selectable: bool,
    /// `None` until a `STATUS` filled them in.
    pub(crate) unread: Option<u32>,
    pub(crate) total: Option<u32>,
}

impl FolderInfo {
    /// Split a mailbox path into its parent path and its own last segment,
    /// using the delimiter the server reported for it. A mailbox at the top
    /// level has no parent.
    pub(crate) fn parent_path(&self) -> Option<&str> {
        let delim = self.delimiter.as_deref()?;
        if delim.is_empty() {
            return None;
        }
        let (parent, _) = self.path.rsplit_once(delim)?;
        (!parent.is_empty()).then_some(parent)
    }
}

/// The row projection of a message — everything a list needs, and nothing
/// that costs a body fetch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct EnvelopeRow {
    pub(crate) uid: u32,
    pub(crate) uid_validity: u32,
    pub(crate) subject: String,
    /// Display form of the first `From` address: `Name <addr>`, or the bare
    /// address when the header carries no display name.
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) date: Option<DateTime<FixedOffset>>,
    pub(crate) size: u32,
    pub(crate) seen: bool,
    pub(crate) flagged: bool,
    pub(crate) answered: bool,
    pub(crate) draft: bool,
    /// How many parts the BODYSTRUCTURE describes as attachments.
    pub(crate) attachments: u32,
}

/// One attachment below a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AttachmentInfo {
    /// IMAP section path of the part (`"2"`, `"2.1"`), also its node id tail.
    pub(crate) part: String,
    pub(crate) filename: String,
    pub(crate) content_type: String,
    pub(crate) size: u32,
}

/// A message body, already reduced to what the preview renders.
#[derive(Clone, Debug, Default)]
pub(crate) struct MessageBody {
    /// Plain text — the `text/plain` alternative where one exists, else the
    /// HTML alternative rendered down to text.
    pub(crate) text: String,
    pub(crate) attachments: Vec<AttachmentInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(path: &str, delim: Option<&str>) -> FolderInfo {
        FolderInfo {
            path: path.into(),
            name: path.into(),
            delimiter: delim.map(Into::into),
            selectable: true,
            ..Default::default()
        }
    }

    /// The delimiter is per-server and per-mailbox: `/` on one, `.` on the
    /// next. Hard-coding either is how a folder tree ends up flat on half the
    /// accounts.
    #[test]
    fn the_parent_is_read_with_the_servers_own_delimiter() {
        assert_eq!(
            folder("INBOX/Projects", Some("/")).parent_path(),
            Some("INBOX")
        );
        assert_eq!(
            folder("INBOX.Projects", Some(".")).parent_path(),
            Some("INBOX")
        );
        assert_eq!(folder("INBOX.Projects", Some("/")).parent_path(), None);
        assert_eq!(folder("INBOX", Some("/")).parent_path(), None);
        assert_eq!(folder("INBOX/Projects", None).parent_path(), None);
    }
}
