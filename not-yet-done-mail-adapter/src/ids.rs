//! Node ids, and the one place that knows how to read them.
//!
//! One instance serves many accounts, so **every** id below the root carries
//! its account as its first segment. That is what makes `get_by_id` work
//! without an ambient "current account" that could drift out of step with the
//! cursor: the id alone says which connection to ask.
//!
//! ```text
//! account      work
//! folder       work/INBOX/Projects
//! message      work/INBOX#1699999999.4711
//! attachment   work/INBOX#1699999999.4711/part/2.1
//! ```
//!
//! The `#<uidvalidity>.<uid>` half is deliberate: IMAP identifies a message by
//! the pair, and a server that renumbers a mailbox bumps its UIDVALIDITY. With
//! the value in the id, a stale bookmark fails to resolve and says so, instead
//! of silently opening whatever message now wears that UID.

/// Marker between a folder path and the message coordinates.
const MSG_SEP: char = '#';
/// Marker between a message id and a MIME part path.
const PART_SEP: &str = "/part/";

/// A message's address inside its account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MessageId {
    pub(crate) account: String,
    pub(crate) folder: String,
    pub(crate) uid_validity: u32,
    pub(crate) uid: u32,
}

impl MessageId {
    pub(crate) fn encode(&self) -> String {
        format!(
            "{}/{}{MSG_SEP}{}.{}",
            self.account, self.folder, self.uid_validity, self.uid
        )
    }

    /// The folder node this message hangs under.
    pub(crate) fn folder_id(&self) -> String {
        folder_id(&self.account, &self.folder)
    }
}

/// `work/INBOX` → `("work", "INBOX")`. The account is everything before the
/// first `/`; the rest is the mailbox path **verbatim**, delimiters included,
/// because the server's delimiter is its own business.
pub(crate) fn split_folder_id(id: &str) -> Option<(&str, &str)> {
    let (account, path) = id.split_once('/')?;
    if account.is_empty() || path.is_empty() {
        return None;
    }
    Some((account, path))
}

pub(crate) fn folder_id(account: &str, path: &str) -> String {
    format!("{account}/{path}")
}

/// Parse a message id. Read from the right: a mailbox name may itself contain
/// `#`, but the coordinates never do.
pub(crate) fn parse_message_id(id: &str) -> Option<MessageId> {
    let (head, coords) = id.rsplit_once(MSG_SEP)?;
    let (account, folder) = split_folder_id(head)?;
    let (validity, uid) = coords.split_once('.')?;
    Some(MessageId {
        account: account.to_string(),
        folder: folder.to_string(),
        uid_validity: validity.parse().ok()?,
        uid: uid.parse().ok()?,
    })
}

/// Parse an attachment id into its message and the MIME part path
/// (`"2.1"`, the IMAP section path of the part).
pub(crate) fn parse_attachment_id(id: &str) -> Option<(MessageId, String)> {
    let (msg, part) = id.rsplit_once(PART_SEP)?;
    if part.is_empty() {
        return None;
    }
    Some((parse_message_id(msg)?, part.to_string()))
}

pub(crate) fn attachment_id(message_id: &str, part: &str) -> String {
    format!("{message_id}{PART_SEP}{part}")
}

/// Which account an id belongs to, whatever kind of id it is. The root has
/// none.
pub(crate) fn account_of(id: &str) -> Option<&str> {
    if id.is_empty() || id == crate::adapter::ROOT_ID {
        return None;
    }
    Some(id.split_once('/').map(|(a, _)| a).unwrap_or(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_id_round_trips() {
        let m = MessageId {
            account: "work".into(),
            folder: "INBOX/Projects".into(),
            uid_validity: 1699999999,
            uid: 4711,
        };
        let encoded = m.encode();
        assert_eq!(encoded, "work/INBOX/Projects#1699999999.4711");
        assert_eq!(parse_message_id(&encoded), Some(m.clone()));
        assert_eq!(m.folder_id(), "work/INBOX/Projects");
    }

    /// Mailbox names are the server's, and some of them contain `#` (public
    /// namespaces do it routinely). Reading the coordinates from the right
    /// keeps those folders addressable.
    #[test]
    fn a_folder_name_may_contain_the_separator() {
        let id = "work/#shared/Team#1699999999.7";
        let m = parse_message_id(id).expect("parses");
        assert_eq!(m.folder, "#shared/Team");
        assert_eq!(m.uid, 7);
    }

    #[test]
    fn an_attachment_id_carries_its_message() {
        let msg = "work/INBOX#1.2";
        let id = attachment_id(msg, "2.1");
        let (parsed, part) = parse_attachment_id(&id).expect("parses");
        assert_eq!(parsed.encode(), msg);
        assert_eq!(part, "2.1");
    }

    #[test]
    fn a_malformed_id_is_none_not_a_panic() {
        assert!(parse_message_id("work/INBOX").is_none());
        assert!(parse_message_id("work/INBOX#nope").is_none());
        assert!(parse_message_id("#1.2").is_none());
        assert!(split_folder_id("work").is_none());
    }

    #[test]
    fn every_id_names_its_account() {
        assert_eq!(account_of("work"), Some("work"));
        assert_eq!(account_of("work/INBOX"), Some("work"));
        assert_eq!(account_of("work/INBOX#1.2"), Some("work"));
        assert_eq!(account_of(crate::adapter::ROOT_ID), None);
    }
}
