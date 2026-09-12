//! One account's sending side: who it sends as, where the draft lives, and
//! what happens to the message after SMTP has taken it.
//!
//! This is the part of composing that is *account* shaped rather than
//! *message* shaped, which is why it does not sit in [`super::render`]: the
//! identity in `From`, the credentials for submission, the folder the copy is
//! filed in and the draft on disk all belong to a mailbox and not to any one
//! mail. A reply and a new message differ in what they quote, and in nothing
//! else — so both end up in [`Outbox::deliver`].

use std::path::PathBuf;
use std::sync::Arc;

use not_yet_done_content::EditorPrep;

use crate::compose::buffer::{self, Headers, QuoteState};
use crate::compose::render::Composition;
use crate::compose::{quote, render, send};
use crate::config::{AccountConfig, ComposeFormat, QuoteImages, SmtpConfig};
use crate::credentials::AccountCredentials;
use crate::error::{MailError, MailResult};
use crate::ids::MessageId;
use crate::imap::conn::Connection;
use crate::mime::Original;

/// The compose settings written once on the instance, inherited by every
/// account that names none of its own.
#[derive(Debug)]
pub(crate) struct ComposeDefaults {
    pub(crate) format: ComposeFormat,
    pub(crate) images: QuoteImages,
    /// The line above a quoted original, with `{date}`, `{name}` and
    /// `{address}` still in it.
    pub(crate) attribution: String,
}

impl Default for ComposeDefaults {
    fn default() -> Self {
        Self {
            format: ComposeFormat::default(),
            images: QuoteImages::default(),
            attribution: quote::DEFAULT_ATTRIBUTION.to_string(),
        }
    }
}

/// Everything one account needs to put a message on the wire.
pub(crate) struct Outbox {
    account: Arc<AccountConfig>,
    /// The credentials submission logs in with — the account's own unless the
    /// `smtp:` block carries an `auth:` of its own.
    creds: Arc<AccountCredentials>,
    mechanism: String,
    defaults: Arc<ComposeDefaults>,
    /// `<instance data>/drafts`. One directory per account below it, so two
    /// accounts replying to the same message do not share a buffer.
    drafts: PathBuf,
    /// The account's IMAP connection — for the copy in Sent and the
    /// `\Answered` flag, both of which happen *after* the send.
    conn: Connection,
}

impl Outbox {
    pub(crate) fn new(
        account: Arc<AccountConfig>,
        creds: Arc<AccountCredentials>,
        mechanism: String,
        defaults: Arc<ComposeDefaults>,
        drafts: PathBuf,
        conn: Connection,
    ) -> Self {
        Self {
            account,
            creds,
            mechanism,
            defaults,
            drafts,
            conn,
        }
    }

    /// The `From` line: the display name the account sends under, and its
    /// address.
    ///
    /// An account without an `address:` can read mail perfectly well and
    /// cannot send any — so the refusal names the key that is missing, at
    /// the moment the editor would have opened rather than after the user
    /// has written a page of text.
    pub(crate) fn identity(&self) -> MailResult<String> {
        let address = self.address()?;
        let name = self
            .account
            .smtp
            .as_ref()
            .and_then(|s| s.from_name.as_deref())
            .unwrap_or_else(|| self.account.label());
        Ok(if name.trim().is_empty() || name == address {
            address.to_string()
        } else {
            format!("{name} <{address}>")
        })
    }

    fn address(&self) -> MailResult<&str> {
        self.account.address.as_deref().filter(|a| !a.trim().is_empty()).ok_or_else(|| {
            MailError::Config(format!(
                "account `{}` has no `address:` — a message needs a sender, and the login name is not one",
                self.account.id
            ))
        })
    }

    /// The addresses this account answers as. One today; a list because
    /// "is this message from me?" is the question [`render::reply_recipients`]
    /// asks, and an alias would extend it here and nowhere else.
    pub(crate) fn own_addresses(&self) -> Vec<String> {
        self.account
            .address
            .iter()
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty())
            .collect()
    }

    /// What a *new* message is sent as. A reply decides from the message it
    /// answers instead.
    fn format(&self) -> ComposeFormat {
        self.account.compose_format.unwrap_or(self.defaults.format)
    }

    fn images(&self) -> QuoteImages {
        self.account.quote_images.unwrap_or(self.defaults.images)
    }

    pub(crate) fn attribution(&self, original: &Original) -> String {
        quote::attribution(&self.defaults.attribution, original)
    }

    fn smtp(&self) -> MailResult<&SmtpConfig> {
        self.account.smtp.as_ref().ok_or_else(|| {
            MailError::Config(format!(
                "account `{}` has no `smtp:` block — it can read mail but not send any",
                self.account.id
            ))
        })
    }

    /// Where a draft of this account lives. The name is the message it
    /// answers (or `compose`), so re-opening the editor finds the text that
    /// was there rather than starting over.
    fn draft_file(&self, name: &str) -> PathBuf {
        self.drafts
            .join(safe(&self.account.id))
            .join(format!("{}.md", safe(name)))
    }

    /// The buffer the editor opens on: a draft that is still on disk, else a
    /// freshly rendered one.
    ///
    /// The frontend writes `template` to `file_path`, so returning the file's
    /// own content is what makes resuming work — anything else would overwrite
    /// the text whose send just failed.
    ///
    /// An account that cannot send is turned away here rather than at
    /// [`Self::deliver`]: the editor is where the writing happens, and finding
    /// out afterwards that this mailbox was never able to send it is the one
    /// moment where the answer arrives too late to be of use.
    pub(crate) fn prep(
        &self,
        name: &str,
        headers: &Headers,
        quoted: Option<&str>,
    ) -> MailResult<EditorPrep> {
        self.smtp()?;
        let path = self.draft_file(name);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| MailError::Config(format!("create {}: {e}", dir.display())))?;
        }
        let fresh = buffer::render(headers, "", quoted);
        let template = match std::fs::read_to_string(&path) {
            Ok(kept) if !kept.trim().is_empty() => kept,
            _ => fresh,
        };
        Ok(EditorPrep {
            template,
            version: String::new(),
            suffix: ".md".to_string(),
            file_path: Some(path),
            args: Default::default(),
        })
    }

    /// Read the saved buffer, and — if it holds together — send it.
    ///
    /// `original` is the message being answered: it decides the format, it
    /// supplies the quote that travels, and it is where the threading headers
    /// come from. `answering` is the same message's coordinates, which is a
    /// separate argument because a reply to a message that has since been
    /// renumbered still sends; only its flag does not stick.
    pub(crate) async fn deliver(
        &self,
        name: &str,
        text: &str,
        original: Option<&Original>,
        answering: Option<&MessageId>,
    ) -> MailResult<String> {
        let attribution = original.map(|o| self.attribution(o)).unwrap_or_default();
        // Judged against a freshly rendered quote rather than against the
        // template the editor was handed: a resumed draft *is* that template,
        // so comparing the two would call an edit that was already refused
        // once "unchanged" the second time round.
        let expected = original.map(|o| quote::quoted_text(o, &attribution));
        let draft = buffer::parse_with_quote(text, expected.as_deref())?;
        if draft.quote == QuoteState::Modified {
            return Err(MailError::Draft(format!(
                "the quoted original was edited, so nothing was sent — what travels is the \
                 sender's own markup, not the text below the marker, and those edits would \
                 be dropped without a trace. Move them above the `{}` line (or delete the \
                 line to reply without a quote); the draft is kept.",
                buffer::QUOTE_MARKER
            )));
        }
        if draft.body.trim().is_empty() {
            return Err(MailError::Draft(
                "the message is empty — write something above the quote".into(),
            ));
        }
        self.check_sender(&draft.headers.from)?;

        let format = match original {
            // The rule from the plan: markup as soon as the original had any,
            // even when a plain alternative sat next to it.
            Some(o) if o.html.is_some() => ComposeFormat::Html,
            Some(_) => ComposeFormat::Plain,
            None => self.format(),
        };
        let message = render::build(&Composition {
            headers: &draft.headers,
            body: &draft.body,
            original,
            quote: draft.quote == QuoteState::Intact && original.is_some(),
            attribution: &attribution,
            format,
            images: self.images(),
        })?;

        let smtp = self.smtp()?;
        let fields = self.creds.fields().await?;
        if let Err(e) = send::send(smtp, &self.mechanism, &fields, &message).await {
            // A rejected submission password is a wrong password, and
            // replaying it until the provider locks the account is the one
            // failure the credential layer exists to prevent.
            if matches!(e, MailError::Auth(_)) {
                self.creds.invalidate().await;
            }
            return Err(e);
        }

        // Past this line the mail is gone and nothing may fail the action:
        // "sending failed" after it has left invites a second send.
        let mut report = format!("sent to {}", draft.headers.to.join(", "));
        // Everyone the mail actually went to, said out loud: a `Cc` that is
        // silently left out of the report reads as one that was left out of
        // the envelope.
        for (label, list) in [("cc", &draft.headers.cc), ("bcc", &draft.headers.bcc)] {
            if !list.is_empty() {
                report.push_str(&format!(" ({label} {})", list.join(", ")));
            }
        }
        match self.conn.append_to_sent(message.formatted()).await {
            Ok(folder) => report.push_str(&format!(", copy in {folder}")),
            Err(e) => report.push_str(&format!(" — but the copy could not be filed: {e}")),
        }
        if let Some(msg) = answering {
            if let Err(e) = self
                .conn
                .store_flags(
                    &msg.folder,
                    msg.uid_validity,
                    &[msg.uid],
                    true,
                    "\\Answered",
                )
                .await
            {
                report.push_str(&format!(" — but the original was not flagged: {e}"));
            }
        }
        let path = self.draft_file(name);
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                report.push_str(&format!(" — the draft {} stayed behind", path.display()));
            }
        }
        Ok(report)
    }

    /// The `From` in the buffer has to be this account's.
    ///
    /// Editing it is how one *would* pick another mailbox to send from, and
    /// that is deliberately not what happens: the node the action ran on names
    /// its account, and sending somebody's mail from the wrong mailbox because
    /// a display name was retyped is not a mistake worth being clever about.
    fn check_sender(&self, from: &str) -> MailResult<()> {
        let mine = self.address()?;
        let written = address_of(from);
        if written.eq_ignore_ascii_case(mine) {
            return Ok(());
        }
        Err(MailError::Draft(format!(
            "`From: {from}` is not the address of account `{}` (`{mine}`) — a message is sent \
             by the mailbox it was written in; open the other account's subtab to send as it",
            self.account.id
        )))
    }
}

/// One path component out of a name that was never meant to be one: a
/// message id carries the folder path and the account, both of which may hold
/// anything the server allows.
fn safe(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.chars().all(|c| c == '.') {
        return "draft".to_string();
    }
    safe
}

/// The bare address out of a `Name <a@b>` line, or the line itself.
fn address_of(value: &str) -> &str {
    match (value.find('<'), value.rfind('>')) {
        (Some(open), Some(close)) if close > open + 1 => value[open + 1..close].trim(),
        _ => value.trim(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(value: &str) -> String {
        address_of(value).to_string()
    }

    #[test]
    fn an_address_is_read_out_of_a_display_name() {
        assert_eq!(
            address("Anna B <anna@example.invalid>"),
            "anna@example.invalid"
        );
        assert_eq!(address("  anna@example.invalid "), "anna@example.invalid");
        assert_eq!(address("<anna@example.invalid>"), "anna@example.invalid");
    }
}
