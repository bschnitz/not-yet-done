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
    /// Build a folder from what `LIST` said about it. The path is kept
    /// **verbatim** — it goes straight back to the server in `SELECT` and
    /// `STATUS` — while the label is the last segment, decoded out of
    /// modified UTF-7 so the tree shows `Entwürfe` and not `Entw&APw-rfe`.
    pub(crate) fn from_wire(path: &str, delimiter: Option<&str>, selectable: bool) -> Self {
        let last = delimiter
            .filter(|d| !d.is_empty())
            .and_then(|d| path.rsplit(d).next())
            .unwrap_or(path);
        Self {
            path: path.to_string(),
            name: crate::mutf7::decode(last),
            delimiter: delimiter.map(str::to_string),
            selectable,
            unread: None,
            total: None,
        }
    }

    /// Whether one of the account's `exclude_folders` entries hides this
    /// mailbox. A bare path hides exactly that folder; a trailing `*` (with or
    /// without a delimiter before it) hides the subtree below it as well —
    /// which is how a server-side archive of a thousand folders is kept out of
    /// the tree without naming each one.
    pub(crate) fn is_excluded(&self, patterns: &[String]) -> bool {
        patterns.iter().any(|p| self.matches_exclude(p.trim()))
    }

    fn matches_exclude(&self, pattern: &str) -> bool {
        let Some(prefix) = pattern.strip_suffix('*') else {
            return self.path == pattern;
        };
        // `INBOX/*`, `INBOX.*` and `INBOX` all mean the same subtree, so the
        // delimiter is trimmed rather than required: the user writing the
        // pattern does not know which one this server uses.
        let prefix = self
            .delimiter
            .as_deref()
            .filter(|d| !d.is_empty())
            .and_then(|d| prefix.strip_suffix(d))
            .unwrap_or(prefix);
        if prefix.is_empty() {
            return true;
        }
        if self.path == prefix {
            return true;
        }
        let Some(rest) = self.path.strip_prefix(prefix) else {
            return false;
        };
        // Only a real child counts: `Archive2` must survive `Archive*`.
        match self.delimiter.as_deref().filter(|d| !d.is_empty()) {
            Some(delim) => rest.starts_with(delim),
            None => true,
        }
    }

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

/// Order the folder list the way a mail client shows it: `INBOX` first —
/// it is the one folder every account has and the one every user looks at —
/// then the rest alphabetically. Sorting by the full path also puts every
/// parent in front of its children, which is what the tree builder needs.
pub(crate) fn sort_for_display(folders: &mut [FolderInfo]) {
    folders.sort_by(|a, b| {
        let key = |f: &FolderInfo| (!f.path.eq_ignore_ascii_case("INBOX"), f.path.to_lowercase());
        key(a).cmp(&key(b))
    });
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
    /// The wire name is the server's business; the label is the user's. Both
    /// have to survive: `SELECT` needs the encoded path back verbatim.
    #[test]
    fn the_label_is_the_decoded_last_segment_but_the_path_stays_verbatim() {
        let f = FolderInfo::from_wire("INBOX/Entw&APw-rfe", Some("/"), true);
        assert_eq!(f.path, "INBOX/Entw&APw-rfe");
        assert_eq!(f.name, "Entwürfe");

        // A flat server names no delimiter — then the whole name is the label.
        let flat = FolderInfo::from_wire("Sent Items", None, true);
        assert_eq!(flat.name, "Sent Items");
    }

    #[test]
    fn an_exclude_pattern_hides_a_folder_or_a_whole_subtree() {
        let archive = FolderInfo::from_wire("Archive", Some("/"), true);
        let below = FolderInfo::from_wire("Archive/2019", Some("/"), true);
        let sibling = FolderInfo::from_wire("Archive2", Some("/"), true);

        let exact = vec!["Archive".to_string()];
        assert!(archive.is_excluded(&exact));
        assert!(
            !below.is_excluded(&exact),
            "an exact pattern is not a subtree"
        );

        // The user writing the pattern does not know the server's delimiter,
        // so all three spellings mean the same subtree.
        for pattern in ["Archive*", "Archive/*"] {
            let p = vec![pattern.to_string()];
            assert!(archive.is_excluded(&p), "{pattern}");
            assert!(below.is_excluded(&p), "{pattern}");
            assert!(!sibling.is_excluded(&p), "{pattern} must not eat Archive2");
        }
    }

    #[test]
    fn inbox_leads_and_parents_precede_their_children() {
        let mut folders = vec![
            FolderInfo::from_wire("Sent", Some("/"), true),
            FolderInfo::from_wire("Archive/2019", Some("/"), true),
            FolderInfo::from_wire("INBOX", Some("/"), true),
            FolderInfo::from_wire("Archive", Some("/"), true),
        ];
        sort_for_display(&mut folders);
        let paths: Vec<&str> = folders.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["INBOX", "Archive", "Archive/2019", "Sent"]);
    }

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
