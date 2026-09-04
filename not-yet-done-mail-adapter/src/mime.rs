//! MIME, once the bytes have arrived.
//!
//! IMAP hands over a message — or one part of one — exactly as it travelled:
//! folded headers, a transfer encoding, and a charset that is very often not
//! UTF-8. Turning that into something a terminal can show is
//! [`mail_parser`]'s job, not a hand-rolled one, so everything here is a thin
//! shaping layer around it.
//!
//! The module deliberately knows nothing about IMAP: it takes bytes and
//! returns text or bytes, which is what makes it testable without a server.

use mail_parser::{Address, Message, MessageParser, MimeHeaders, PartType};

/// The readable text of a whole message source.
///
/// `body_text(0)` is mail-parser's own preference order, and it is the one a
/// mail client wants: the `text/plain` alternative where the sender supplied
/// one, otherwise the HTML alternative rendered down to text. A message with
/// neither (a bare attachment, a calendar invitation) has no body, and says so
/// by being empty rather than by inventing a placeholder.
///
/// Line endings are normalised on the way out. Mail travels with CRLF, and a
/// stray `\r` in a terminal pane is a visible artefact.
pub(crate) fn body_text(raw: &[u8]) -> String {
    MessageParser::default()
        .parse(raw)
        .and_then(|msg| msg.body_text(0).map(|t| t.into_owned()))
        .map(|text| text.replace("\r\n", "\n"))
        .unwrap_or_default()
}

/// One part the HTML body points at with `cid:` — the sender's own images: a
/// logo under a signature, a screenshot pasted into the text. They are not
/// attachments (they have no filename and nothing lists them), they are the
/// body, and without them half an HTML mail is boxes with crosses in.
pub(crate) struct InlinePart {
    /// The `Content-ID`, angle brackets stripped — what the markup spells
    /// after `cid:`.
    pub id: String,
    /// The name the part is written under. Derived from the id, so the
    /// markup can be pointed at it, prefixed with the part's number because
    /// two ids may sanitise down to the same string.
    pub file_name: String,
    /// `type/subtype` as the sender declared it. A viewer that writes the
    /// part to disk does not need it — the extension carries it — but a
    /// reply that carries the part back out does: a MIME part without a
    /// content type is a download prompt instead of an image.
    pub content_type: Option<String>,
    pub bytes: Vec<u8>,
}

/// The HTML half of a message: the markup exactly as the sender wrote it,
/// plus the parts it references.
///
/// *Exactly as the sender wrote it* is the point. [`body_text`] hands back
/// the HTML flattened to text, which is right for a terminal pane and wrong
/// for a browser — the tables, the headings and the images are the message.
/// Nothing here sanitises: this module knows MIME, not what is safe to put
/// in front of a renderer, and a viewer that trusts unfiltered mail markup
/// would be wrong whether the filtering happened here or not.
pub(crate) struct HtmlBody {
    pub html: String,
    pub inline: Vec<InlinePart>,
}

/// The message's own `text/html` part, or `None` when it has none.
///
/// Deliberately *not* mail-parser's `body_html(0)`: that one converts a
/// plain-text body to markup when there is no HTML part, so it can never
/// answer "this message is plain text" — which is exactly the question the
/// caller has to ask before deciding what to render.
fn html_of(msg: &Message) -> Option<HtmlBody> {
    let html = msg
        .html_bodies()
        .find(|part| part.is_text_html())
        .and_then(|part| part.text_contents())?
        .to_string();
    let inline = msg
        .parts
        .iter()
        .enumerate()
        .filter_map(|(index, part)| {
            let id = part.content_id()?.trim().trim_matches(['<', '>']).trim();
            if id.is_empty() || part.is_text_html() {
                return None;
            }
            Some(InlinePart {
                file_name: inline_file_name(index, id, part.content_type()),
                content_type: part.content_type().map(content_type_of),
                id: id.to_string(),
                bytes: part.contents().to_vec(),
            })
        })
        .collect();
    Some(HtmlBody { html, inline })
}

/// Everything an external viewer needs from one message, from one parse.
///
/// The header fields are here rather than taken from the row the listing
/// already holds because that row is an IMAP envelope: it has no `Cc`, and it
/// counts attachments instead of naming them. A reader wants both.
pub(crate) struct Export {
    pub subject: String,
    pub from: String,
    pub to: String,
    pub cc: String,
    /// RFC 3339, or empty when the message carries no readable `Date`.
    pub date: String,
    pub attachments: Vec<String>,
    pub html: Option<HtmlBody>,
    /// The plain-text alternative, when the message has one. Never the HTML
    /// flattened down: a viewer that got both would have no way to tell a
    /// real text part from a generated one.
    pub text: Option<String>,
}

/// One message, taken apart for a renderer.
pub(crate) fn export(raw: &[u8]) -> Export {
    let Some(msg) = MessageParser::default().parse(raw) else {
        return Export {
            subject: String::new(),
            from: String::new(),
            to: String::new(),
            cc: String::new(),
            date: String::new(),
            attachments: Vec::new(),
            html: None,
            text: None,
        };
    };
    let text = msg
        .text_bodies()
        .find(|part| part.is_text() && !part.is_text_html())
        .and_then(|part| part.text_contents())
        .map(|text| text.replace("\r\n", "\n"));
    Export {
        subject: msg.subject().unwrap_or_default().to_string(),
        from: addresses(msg.from()),
        to: addresses(msg.to()),
        cc: addresses(msg.cc()),
        date: msg.date().map(|d| d.to_rfc3339()).unwrap_or_default(),
        attachments: msg
            .attachments()
            .filter_map(|part| part.attachment_name().map(str::to_string))
            .collect(),
        html: html_of(&msg),
        text,
    }
}

/// An address header as a reader writes it: `Name <local@host>`, joined with
/// commas. A group (`undisclosed-recipients:;`) contributes its members, which
/// is what [`Address::iter`] already walks.
fn addresses(addr: Option<&Address>) -> String {
    let Some(addr) = addr else {
        return String::new();
    };
    addr.iter()
        .map(|one| match (one.name(), one.address()) {
            (Some(name), Some(address)) => format!("{name} <{address}>"),
            (Some(name), None) => name.to_string(),
            (None, Some(address)) => address.to_string(),
            (None, None) => String::new(),
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// `<index>-<id>.<ext>`, with everything a file system objects to folded
/// away. The extension comes from the part's MIME subtype so the browser and
/// the image viewer recognise the file; a part without a usable type gets
/// none rather than a made-up one.
fn inline_file_name(index: usize, id: &str, ctype: Option<&mail_parser::ContentType>) -> String {
    let safe: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let ext = ctype
        .and_then(|c| c.subtype())
        .map(|s| {
            s.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .map(|s| format!(".{s}"))
        .unwrap_or_default();
    format!("{index}-{safe}{ext}")
}

/// Point the markup at the files instead of at the message: every
/// `cid:<id>` an inline part answers to becomes `<dir>/<file name>`.
///
/// A reference nothing answers to is left alone. It cannot be resolved
/// either way, and rewriting it to a path that does not exist would only
/// hide that from whoever has to explain the missing image.
pub(crate) fn link_inline(html: &str, inline: &[InlinePart], dir: &str) -> String {
    // Lowered once, and only to find the scheme: `to_ascii_lowercase` keeps
    // every byte where it was, so an index into the copy indexes the original.
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut pos = 0;
    while let Some(offset) = lower[pos..].find("cid:") {
        let at = pos + offset;
        out.push_str(&html[pos..at]);
        let after = &html[at + 4..];
        let end = after
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '>' | ')' | '\\'))
            .unwrap_or(after.len());
        let reference = &after[..end];
        match inline
            .iter()
            .find(|part| part.id.eq_ignore_ascii_case(reference))
        {
            Some(part) => out.push_str(&format!("{dir}/{}", part.file_name)),
            None => out.push_str(&html[at..at + 4 + end]),
        }
        pos = at + 4 + end;
    }
    out.push_str(&html[pos..]);
    out
}

/// The decoded bytes of one MIME part, from the two things a
/// `BODY.PEEK[<part>.MIME]` + `BODY.PEEK[<part>]` fetch returns.
///
/// The headers are what makes this possible at all: on their own the bytes of
/// a part say nothing about their transfer encoding, so base64 would be
/// written to disk verbatim. Gluing the two halves back together produces a
/// one-part message, and the parser that already handles every encoding
/// decodes it.
///
/// Without headers — a server that answered the body but not the `.MIME`
/// section — the bytes are passed through unchanged. That is the honest
/// fallback: possibly still encoded, but never mangled.
pub(crate) fn decode_part(mime_headers: &[u8], body: &[u8]) -> Vec<u8> {
    if mime_headers.is_empty() {
        return body.to_vec();
    }
    let mut synthetic = Vec::with_capacity(mime_headers.len() + body.len() + 2);
    synthetic.extend_from_slice(mime_headers);
    // The header block ends with a blank line. Servers usually include the
    // trailing CRLF in the `.MIME` section, so add only what is missing.
    if !synthetic.ends_with(b"\r\n") {
        synthetic.extend_from_slice(b"\r\n");
    }
    synthetic.extend_from_slice(b"\r\n");
    synthetic.extend_from_slice(body);

    let Some(parsed) = MessageParser::default().parse(&synthetic) else {
        return body.to_vec();
    };
    match parsed.parts.first().map(|p| &p.body) {
        Some(PartType::Text(text)) | Some(PartType::Html(text)) => text.as_bytes().to_vec(),
        Some(PartType::Binary(bytes)) | Some(PartType::InlineBinary(bytes)) => bytes.to_vec(),
        _ => body.to_vec(),
    }
}

/// `type/subtype`, lowercased by the parser already.
fn content_type_of(ctype: &mail_parser::ContentType) -> String {
    match ctype.subtype() {
        Some(sub) => format!("{}/{}", ctype.ctype(), sub),
        None => ctype.ctype().to_string(),
    }
}

/// One address, still in two pieces.
///
/// [`addresses`] joins them into the line a reader sees; a reply has to hand
/// the address itself to a mail builder and compare it against the accounts
/// the instance carries, and neither survives being folded into a string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Contact {
    pub name: Option<String>,
    pub address: String,
}

impl Contact {
    /// `Name <local@host>`, or the bare address when the sender wrote no name.
    pub fn display(&self) -> String {
        match &self.name {
            Some(name) if !name.trim().is_empty() => format!("{name} <{}>", self.address),
            _ => self.address.clone(),
        }
    }
}

fn contacts(addr: Option<&Address>) -> Vec<Contact> {
    let Some(addr) = addr else {
        return Vec::new();
    };
    addr.iter()
        .filter_map(|one| {
            Some(Contact {
                name: one.name().map(str::to_string),
                address: one.address()?.to_string(),
            })
        })
        .collect()
}

/// Everything a reply needs from the message it answers.
///
/// Not [`Export`] with more fields: an export describes a message *to a
/// reader*, and a reader has no use for a `References` chain, while a reply
/// cannot be threaded without one. The two overlap in the obvious places and
/// are asked different questions.
pub(crate) struct Original {
    pub subject: String,
    /// The `Message-ID`, angle brackets stripped.
    pub message_id: Option<String>,
    /// The chain this message hangs in, oldest first, angle brackets
    /// stripped. Its own id is *not* in here — the reply appends that.
    pub references: Vec<String>,
    pub from: Vec<Contact>,
    /// Where the sender asked to be answered. Beats `from` when present:
    /// that is the entire purpose of the header.
    pub reply_to: Vec<Contact>,
    pub to: Vec<Contact>,
    pub cc: Vec<Contact>,
    /// RFC 3339, or `None` when the message carries no readable `Date`.
    pub date: Option<String>,
    pub html: Option<HtmlBody>,
    pub text: Option<String>,
}

/// One message, taken apart for an answer.
pub(crate) fn original(raw: &[u8]) -> Original {
    let Some(msg) = MessageParser::default().parse(raw) else {
        return Original {
            subject: String::new(),
            message_id: None,
            references: Vec::new(),
            from: Vec::new(),
            reply_to: Vec::new(),
            to: Vec::new(),
            cc: Vec::new(),
            date: None,
            html: None,
            text: None,
        };
    };
    Original {
        subject: msg.subject().unwrap_or_default().to_string(),
        message_id: msg.message_id().map(bare_id),
        references: reference_chain(&msg),
        from: contacts(msg.from()),
        reply_to: contacts(msg.reply_to()),
        to: contacts(msg.to()),
        cc: contacts(msg.cc()),
        date: msg.date().map(|d| d.to_rfc3339()),
        html: html_of(&msg),
        text: msg
            .text_bodies()
            .find(|part| part.is_text() && !part.is_text_html())
            .and_then(|part| part.text_contents())
            .map(|text| text.replace("\r\n", "\n")),
    }
}

/// `References`, falling back to `In-Reply-To` for the clients that only ever
/// wrote that one. Duplicates are dropped: a chain that repeats an id makes
/// some clients draw the thread twice.
fn reference_chain(msg: &Message) -> Vec<String> {
    let mut chain: Vec<String> = Vec::new();
    let listed = msg
        .references()
        .as_text_list()
        .into_iter()
        .flatten()
        .chain(msg.in_reply_to().as_text_list().into_iter().flatten());
    for id in listed {
        let id = bare_id(id.as_ref());
        if !id.is_empty() && !chain.contains(&id) {
            chain.push(id);
        }
    }
    chain
}

/// A message id without its angle brackets, whichever form the header used.
fn bare_id(id: &str) -> String {
    id.trim().trim_matches(['<', '>']).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The everyday case, and the one a hand-rolled decoder gets wrong: the
    /// body is quoted-printable in a non-UTF-8 charset.
    #[test]
    fn a_plain_body_arrives_decoded_and_in_utf8() {
        let raw = concat!(
            "From: a@example.org\r\n",
            "Subject: Test\r\n",
            "Content-Type: text/plain; charset=ISO-8859-1\r\n",
            "Content-Transfer-Encoding: quoted-printable\r\n",
            "\r\n",
            "Gr=FC=DFe aus M=FCnchen\r\n"
        );
        assert_eq!(body_text(raw.as_bytes()).trim(), "Grüße aus München");
    }

    /// A multipart/alternative must show its plain half, not its markup.
    #[test]
    fn the_plain_alternative_wins_over_the_html_one() {
        let raw = concat!(
            "Content-Type: multipart/alternative; boundary=\"b\"\r\n",
            "\r\n",
            "--b\r\n",
            "Content-Type: text/plain\r\n\r\n",
            "the plain one\r\n",
            "--b\r\n",
            "Content-Type: text/html\r\n\r\n",
            "<p>the html one</p>\r\n",
            "--b--\r\n"
        );
        assert_eq!(body_text(raw.as_bytes()).trim(), "the plain one");
    }

    /// An HTML-only mail is half the mail in an average mailbox. Showing its
    /// tags would be worse than showing nothing, so it is rendered to text.
    #[test]
    fn an_html_only_mail_is_rendered_down_to_text() {
        let raw = concat!(
            "Content-Type: text/html; charset=utf-8\r\n",
            "\r\n",
            "<html><body><p>Hello <b>there</b></p></body></html>\r\n"
        );
        let text = body_text(raw.as_bytes());
        assert!(text.contains("Hello"), "{text:?}");
        assert!(!text.contains("<b>"), "the markup must not reach the pane");
    }

    /// The whole reason `body_html` exists: the browser must get the sender's
    /// markup, not the flattened text the terminal pane shows.
    #[test]
    fn an_html_alternative_arrives_as_markup() {
        let raw = concat!(
            "Content-Type: multipart/alternative; boundary=\"b\"\r\n",
            "\r\n",
            "--b\r\n",
            "Content-Type: text/plain\r\n\r\n",
            "the plain one\r\n",
            "--b\r\n",
            "Content-Type: text/html\r\n\r\n",
            "<p>the <b>html</b> one</p>\r\n",
            "--b--\r\n"
        );
        let body = export(raw.as_bytes()).html.expect("an html part");
        assert!(body.html.contains("<b>html</b>"), "{:?}", body.html);
        assert!(body.inline.is_empty());
    }

    /// A plain-text mail must say it has no HTML rather than hand back a
    /// converted body — the caller renders text differently on purpose.
    #[test]
    fn a_plain_mail_has_no_html_body() {
        let raw = "Content-Type: text/plain\r\n\r\nnothing but text\r\n";
        assert!(export(raw.as_bytes()).html.is_none());
    }

    /// The signature logo: carried as a part, referenced as `cid:`, and
    /// nowhere in the attachment list.
    #[test]
    fn an_inline_image_comes_along_and_the_markup_points_at_the_file() {
        let raw = concat!(
            "Content-Type: multipart/related; boundary=\"r\"\r\n",
            "\r\n",
            "--r\r\n",
            "Content-Type: text/html\r\n\r\n",
            "<img src=3D\"cid:logo@example\"><img src=\"CID:logo@example\">\r\n",
            "--r\r\n",
            "Content-Type: image/png\r\n",
            "Content-ID: <logo@example>\r\n",
            "Content-Transfer-Encoding: base64\r\n\r\n",
            "aGk=\r\n",
            "--r--\r\n"
        );
        let body = export(raw.as_bytes()).html.expect("an html part");
        assert_eq!(body.inline.len(), 1);
        let part = &body.inline[0];
        assert_eq!(part.id, "logo@example");
        assert!(
            part.file_name.ends_with("-logo_example.png"),
            "{}",
            part.file_name
        );
        assert_eq!(part.bytes, b"hi");
        let linked = link_inline(&body.html, &body.inline, "inline");
        assert_eq!(
            linked
                .matches(&format!("inline/{}", part.file_name))
                .count(),
            2
        );
        assert!(!linked.to_ascii_lowercase().contains("cid:"), "{linked}");
    }

    /// One parse has to answer every question a viewer asks — including the
    /// two the envelope row cannot: who was copied in, and what the files are
    /// called.
    #[test]
    fn an_export_carries_the_headers_the_row_does_not() {
        let raw = concat!(
            "From: Ada <ada@example.org>\r\n",
            "To: Bob <bob@example.org>, carol@example.org\r\n",
            "Cc: Dan <dan@example.org>\r\n",
            "Subject: Angebot\r\n",
            "Date: Tue, 1 Sep 2026 10:00:00 +0200\r\n",
            "Content-Type: multipart/mixed; boundary=\"m\"\r\n",
            "\r\n",
            "--m\r\n",
            "Content-Type: text/plain\r\n\r\n",
            "hello\r\n",
            "--m\r\n",
            "Content-Type: application/pdf; name=\"offer.pdf\"\r\n",
            "Content-Disposition: attachment; filename=\"offer.pdf\"\r\n\r\n",
            "%PDF\r\n",
            "--m--\r\n"
        );
        let export = export(raw.as_bytes());
        assert_eq!(export.subject, "Angebot");
        assert_eq!(export.from, "Ada <ada@example.org>");
        assert_eq!(export.to, "Bob <bob@example.org>, carol@example.org");
        assert_eq!(export.cc, "Dan <dan@example.org>");
        assert!(
            export.date.starts_with("2026-09-01T10:00:00"),
            "{}",
            export.date
        );
        assert_eq!(export.attachments, ["offer.pdf"]);
        assert_eq!(export.text.as_deref(), Some("hello"));
        assert!(export.html.is_none(), "a plain mail has no markup to show");
    }

    /// A reference nothing answers to stays as it is: a path that resolves to
    /// nothing would hide the missing part instead of showing it.
    #[test]
    fn an_unanswered_cid_reference_is_left_alone() {
        let html = "<img src=\"cid:gone@example\"> and cid: on its own";
        assert_eq!(link_inline(html, &[], "inline"), html);
    }

    /// A message that is only an attachment has no body — and an empty pane
    /// is a truer answer than a made-up one.
    #[test]
    fn a_message_without_a_text_part_has_no_body() {
        let raw = concat!(
            "Content-Type: application/pdf; name=\"x.pdf\"\r\n",
            "Content-Transfer-Encoding: base64\r\n",
            "\r\n",
            "aGk=\r\n"
        );
        assert_eq!(body_text(raw.as_bytes()), "");
    }

    /// The whole point of fetching the `.MIME` section alongside the part:
    /// without it the bytes on disk would be the base64, not the file.
    #[test]
    fn a_part_is_decoded_from_its_own_headers() {
        let headers = b"Content-Type: application/pdf; name=\"x.pdf\"\r\n\
                        Content-Transfer-Encoding: base64\r\n";
        assert_eq!(
            decode_part(headers, b"aGVsbG8gd29ybGQ=\r\n"),
            b"hello world"
        );
    }

    /// A part with no encoding at all is passed through byte for byte — an
    /// attachment must never be "helpfully" reshaped.
    #[test]
    fn an_unencoded_part_survives_verbatim() {
        let headers = b"Content-Type: application/octet-stream\r\n";
        let body = b"\x00\x01\x02raw";
        assert_eq!(decode_part(headers, body), body);
        // And with no headers at all there is nothing to decode with, so the
        // bytes are handed on rather than guessed at.
        assert_eq!(decode_part(b"", body), body);
    }
}
