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

use mail_parser::{MessageParser, PartType};

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
        assert_eq!(decode_part(headers, b"aGVsbG8gd29ybGQ=\r\n"), b"hello world");
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
