//! Where the adapter puts the files it hands back to the desktop.
//!
//! Two levels write to disk and both want the same folder: an attachment
//! being opened, and a message being exported as HTML for an external
//! viewer. One directory per message keeps every file of one mail together —
//! the viewer's next/previous then pages through the message — and makes a
//! second look free: what is already there is not fetched again.
//!
//! Nothing here is IMAP. It is the small amount of file-system defensiveness
//! that reading a stranger's mail requires: a filename is the *sender's*
//! text, and it reaches disk through this module.

use std::path::PathBuf;

/// Reduce a string to a safe single path component.
pub(super) fn sanitize_component(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Strip directory parts off a server-supplied filename and make it safe to
/// write. A filename is the *sender's* text: it may name `../` or a drive,
/// and it reaches disk here.
pub(super) fn safe_file_name(filename: &str) -> String {
    let base = filename
        .rsplit(['/', '\\'])
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("file");
    let safe = sanitize_component(base);
    if safe.chars().all(|c| c == '.') {
        return "file".to_string();
    }
    safe
}

/// The per-message directory, created if it does not exist yet.
pub(super) fn message_dir(message_id: &str) -> std::io::Result<PathBuf> {
    let mut dir = std::env::temp_dir();
    dir.push("not_yet_done_mail");
    dir.push(sanitize_component(message_id));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two things a sender must not be able to do: escape the directory,
    /// and name a file that is only dots.
    #[test]
    fn a_hostile_filename_stays_one_harmless_component() {
        assert_eq!(safe_file_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_file_name("C:\\Windows\\x.txt"), "x.txt");
        assert_eq!(safe_file_name(".."), "file");
        assert_eq!(safe_file_name("   "), "file");
    }

    /// Everything a file system might object to becomes an underscore, and
    /// the parts a reader recognises survive.
    #[test]
    fn a_component_keeps_its_readable_characters() {
        assert_eq!(sanitize_component("Re: Angebot (1).pdf"), "Re__Angebot__1_.pdf");
    }
}
