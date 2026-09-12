//! Turning one `FETCH` response into a row.
//!
//! This is where the protocol stops. Above it everything is
//! [`crate::model`]; below it are `async-imap` types. Two jobs live here that
//! look small and are not:
//!
//! - **Headers are not text.** `ENVELOPE` hands back the bytes of the header
//!   as they travelled, so a subject reads `=?UTF-8?Q?Gr=C3=BC=C3=9Fe?=` and
//!   a display name the same. Decoding them is mail-parser's job, not a
//!   hand-rolled one — encoded words nest, split across whitespace and carry
//!   a charset each.
//! - **The attachment count is a walk, not a field.** IMAP describes the MIME
//!   tree in `BODYSTRUCTURE` and nowhere says how many attachments there are;
//!   the number a mail client shows is the result of walking that tree and
//!   deciding what counts.

use async_imap::imap_proto::types::{
    Address, BodyContentCommon, BodyParams, BodyStructure, Envelope,
};
use async_imap::types::{Fetch, Flag};
use chrono::{DateTime, FixedOffset};
use mail_parser::MessageParser;

use crate::model::{AttachmentInfo, EnvelopeRow};

/// Decode one header value that arrived as raw bytes.
///
/// Done by handing mail-parser a one-header message rather than by reaching
/// into its internals: RFC 2047 is folding, adjacent encoded words, a charset
/// per word and a base64/quoted-printable choice per word. The library that
/// already implements all of it should be the one implementing it.
pub(crate) fn decode_header(raw: &[u8]) -> String {
    let raw = String::from_utf8_lossy(raw);
    // Unfold first: a header value may arrive wrapped, and the continuation
    // line's leading whitespace is folding, not content. Feeding the wrap
    // into the synthetic message below would end the header early.
    let single = raw
        .lines()
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join(" ");
    let synthetic = format!("Subject: {single}\r\n\r\n");
    MessageParser::default()
        .parse(synthetic.as_bytes())
        .and_then(|m| m.subject().map(str::to_string))
        .unwrap_or_else(|| single.trim().to_string())
}

/// `Name <user@host>`, or the bare address when the header carries no display
/// name — the form a mail client's From column shows.
pub(crate) fn format_address(addr: &Address<'_>) -> String {
    let mailbox = addr.mailbox.as_deref().map(String::from_utf8_lossy);
    let host = addr.host.as_deref().map(String::from_utf8_lossy);
    let address = match (mailbox, host) {
        (Some(m), Some(h)) => format!("{m}@{h}"),
        (Some(m), None) => m.to_string(),
        // A group syntax entry (`undisclosed-recipients:;`) has no mailbox.
        (None, _) => String::new(),
    };
    let name = addr
        .name
        .as_deref()
        .map(decode_header)
        .filter(|n| !n.trim().is_empty());
    match (name, address.is_empty()) {
        (Some(name), false) => format!("{name} <{address}>"),
        (Some(name), true) => name,
        (None, _) => address,
    }
}

/// The first address of a header, formatted. A list shows one sender, not
/// seven; the full set is on the message itself.
fn first_address(list: Option<&Vec<Address<'_>>>) -> String {
    list.and_then(|l| l.first())
        .map(format_address)
        .unwrap_or_default()
}

/// When the message says it was sent, preferring the `Date:` header over the
/// server's own arrival time — the header is what the sender meant and what
/// every other mail client shows. `INTERNALDATE` is the fallback, because a
/// message with a broken or missing `Date:` still has to sort somewhere.
fn sent_at(
    envelope: Option<&Envelope<'_>>,
    internal: Option<DateTime<FixedOffset>>,
) -> Option<DateTime<FixedOffset>> {
    envelope
        .and_then(|e| e.date.as_deref())
        .map(|raw| String::from_utf8_lossy(raw).trim().to_string())
        .and_then(|raw| DateTime::parse_from_rfc2822(&raw).ok())
        .or(internal)
}

/// Whether one MIME part is what a user would call an attachment.
///
/// `Content-Disposition: attachment` is the answer when the server states it.
/// Plenty of mailers omit the header entirely and only give the part a
/// `name=` parameter, so a filename without a disposition counts too —
/// otherwise half the mail in an average mailbox would show no paperclip.
/// An inline part with no filename (the HTML alternative, an embedded signature
/// image) does not count: it is part of the body being displayed.
fn is_attachment(common: &BodyContentCommon<'_>) -> Option<String> {
    let filename = |params: &BodyParams<'_>| {
        params.as_ref().and_then(|p| {
            p.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("filename") || k.eq_ignore_ascii_case("name"))
                .map(|(_, v)| decode_header(v.as_bytes()))
        })
    };
    match common.disposition.as_ref() {
        Some(d) if d.ty.eq_ignore_ascii_case("attachment") => {
            Some(filename(&d.params).unwrap_or_default())
        }
        Some(d) if d.ty.eq_ignore_ascii_case("inline") => filename(&d.params),
        // No disposition at all: a filename parameter on the content type is
        // then the only thing the sender said about it.
        Some(_) => None,
        None => filename(&common.ty.params),
    }
}

/// Every attachment in a message's MIME tree, with the IMAP section path that
/// addresses it (`"2"`, `"2.1"`) — which is also the tail of its node id.
pub(crate) fn attachments_of(body: &BodyStructure<'_>) -> Vec<AttachmentInfo> {
    let mut out = Vec::new();
    walk(body, "", &mut out);
    out
}

fn walk(body: &BodyStructure<'_>, prefix: &str, out: &mut Vec<AttachmentInfo>) {
    let path = |i: usize| {
        if prefix.is_empty() {
            (i + 1).to_string()
        } else {
            format!("{prefix}.{}", i + 1)
        }
    };
    match body {
        BodyStructure::Multipart { bodies, .. } => {
            for (i, child) in bodies.iter().enumerate() {
                walk(child, &path(i), out);
            }
        }
        BodyStructure::Basic { common, other, .. }
        | BodyStructure::Text { common, other, .. }
        | BodyStructure::Message { common, other, .. } => {
            let Some(filename) = is_attachment(common) else {
                return;
            };
            // A message that is a single part *is* part 1, which is how it
            // has to be addressed in a `BODY[…]` fetch.
            let part = if prefix.is_empty() {
                "1".to_string()
            } else {
                prefix.to_string()
            };
            let content_type = format!("{}/{}", common.ty.ty, common.ty.subtype).to_lowercase();
            out.push(AttachmentInfo {
                filename: if filename.is_empty() {
                    format!("part {part}")
                } else {
                    filename
                },
                part,
                content_type,
                size: other.octets,
            });
        }
    }
}

/// Project one `FETCH` response into a row. `None` when the response carries
/// no UID — without one the message has no address, so there is nothing a row
/// could point at.
pub(crate) fn row_from(fetch: &Fetch, uid_validity: u32) -> Option<EnvelopeRow> {
    let uid = fetch.uid?;
    let envelope = fetch.envelope();
    let mut row = EnvelopeRow {
        uid,
        uid_validity,
        subject: envelope
            .and_then(|e| e.subject.as_deref())
            .map(decode_header)
            .unwrap_or_default(),
        from: first_address(envelope.and_then(|e| e.from.as_ref())),
        to: first_address(envelope.and_then(|e| e.to.as_ref())),
        date: sent_at(envelope, fetch.internal_date()),
        size: fetch.size.unwrap_or(0),
        ..Default::default()
    };
    for flag in fetch.flags() {
        match flag {
            Flag::Seen => row.seen = true,
            Flag::Flagged => row.flagged = true,
            Flag::Answered => row.answered = true,
            Flag::Draft => row.draft = true,
            _ => {}
        }
    }
    row.attachments = fetch
        .bodystructure()
        .map(attachments_of)
        .unwrap_or_default();
    Some(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one thing every German mailbox proves within three rows: a subject
    /// arrives encoded, and a client that prints it raw is unusable.
    #[test]
    fn an_encoded_subject_is_decoded() {
        assert_eq!(decode_header(b"=?UTF-8?Q?Gr=C3=BC=C3=9Fe?="), "Grüße");
        assert_eq!(
            decode_header(b"=?ISO-8859-1?Q?Rechnung_f=FCr_M=E4rz?="),
            "Rechnung für März"
        );
        // Plain ASCII passes through untouched, folding and all.
        assert_eq!(decode_header(b"Re: the\r\n  meeting"), "Re: the meeting");
    }

    #[test]
    fn an_address_reads_as_a_mail_client_shows_it() {
        let addr = |name: Option<&'static str>, mbox: &'static str, host: &'static str| Address {
            name: name.map(|n| n.as_bytes().into()),
            adl: None,
            mailbox: Some(mbox.as_bytes().into()),
            host: Some(host.as_bytes().into()),
        };
        assert_eq!(
            format_address(&addr(Some("=?UTF-8?Q?J=C3=BCrgen?="), "j", "example.org")),
            "Jürgen <j@example.org>"
        );
        assert_eq!(
            format_address(&addr(None, "noreply", "example.org")),
            "noreply@example.org"
        );
    }
}
