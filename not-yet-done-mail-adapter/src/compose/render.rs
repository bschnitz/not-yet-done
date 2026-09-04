//! From the buffer to the message that travels.
//!
//! Two conversions meet here and they are deliberately not symmetric. What
//! the user wrote is Markdown and becomes HTML; the original that is quoted
//! below it is *not* converted back from the text in the buffer — its own
//! markup is taken and wrapped in a `<blockquote>`, which is why a mail
//! thread still reads as a thread after four rounds through four clients.

use lettre::Message;
use lettre::message::header::ContentType;
use lettre::message::{Attachment, MultiPart, SinglePart};

use crate::compose::buffer::Headers;
use crate::compose::quote;
use crate::config::{ComposeFormat, QuoteImages};
use crate::error::{MailError, MailResult};
use crate::mime::Original;

/// The quote styling every client draws and no client keeps in a stylesheet:
/// mail strips `<style>` blocks, so the only styling that survives the trip
/// is written on the element itself.
const QUOTE_STYLE: &str = "border-left: 1px solid #ccc; margin: 0 0 0 .8ex; padding-left: 1ex;";

/// Everything the outgoing message is made of, once the buffer has been read
/// and judged.
pub(crate) struct Composition<'a> {
    pub headers: &'a Headers,
    /// What the user wrote, as Markdown.
    pub body: &'a str,
    /// The message this answers. Threading uses it even when the quote was
    /// dropped — a reply belongs in its thread whether or not it repeats it.
    pub original: Option<&'a Original>,
    /// Whether the original travels back inside the reply.
    pub quote: bool,
    /// The attribution line, already filled in.
    pub attribution: &'a str,
    pub format: ComposeFormat,
    pub images: QuoteImages,
}

/// Build the MIME message.
///
/// The shape is the one every client produces for an HTML mail with a quote:
/// `multipart/alternative` over a `text/plain` half and a `multipart/related`
/// half, the latter holding the markup and the images it points at. A plain
/// message is a single part and nothing more.
pub(crate) fn build(c: &Composition) -> MailResult<Message> {
    let mut builder = Message::builder()
        .from(mailbox(&c.headers.from)?)
        .subject(c.headers.subject.clone())
        .message_id(Some(mint_message_id(&c.headers.from)));
    if c.headers.to.is_empty() {
        return Err(MailError::Draft(
            "the message has no recipient — put an address in the To: line".into(),
        ));
    }
    for address in &c.headers.to {
        builder = builder.to(mailbox(address)?);
    }
    for address in &c.headers.cc {
        builder = builder.cc(mailbox(address)?);
    }
    for address in &c.headers.bcc {
        builder = builder.bcc(mailbox(address)?);
    }
    if let Some(original) = c.original {
        if let Some(id) = &original.message_id {
            builder = builder.in_reply_to(format!("<{id}>"));
        }
        let chain = reference_chain(original);
        if !chain.is_empty() {
            builder = builder.references(chain.join(" "));
        }
    }

    let plain = plain_body(c);
    let message = match c.format {
        ComposeFormat::Plain => builder
            .header(ContentType::TEXT_PLAIN)
            .body(plain)
            .map_err(|e| MailError::Draft(format!("the message could not be assembled: {e}")))?,
        ComposeFormat::Html => {
            let html = SinglePart::html(html_body(c));
            let inline = self::inline_parts(c);
            let markup = if inline.is_empty() {
                MultiPart::alternative()
                    .singlepart(SinglePart::plain(plain))
                    .singlepart(html)
            } else {
                let mut related = MultiPart::related().singlepart(html);
                for part in inline {
                    related = related.singlepart(part);
                }
                MultiPart::alternative()
                    .singlepart(SinglePart::plain(plain))
                    .multipart(related)
            };
            builder
                .multipart(markup)
                .map_err(|e| MailError::Draft(format!("the message could not be assembled: {e}")))?
        }
    };
    Ok(message)
}

/// The text half: what the user wrote, then the original with `> ` in front
/// of every line. Both halves say the same thing, which is the whole promise
/// of `multipart/alternative`.
fn plain_body(c: &Composition) -> String {
    let mut out = c.body.trim_end().to_string();
    if let (true, Some(original)) = (c.quote, c.original) {
        out.push_str("\n\n");
        out.push_str(&quote::quoted_text(original, c.attribution));
    }
    out.push('\n');
    out
}

/// The markup half: the user's Markdown rendered, then the original's own
/// markup inside a `<blockquote>`.
fn html_body(c: &Composition) -> String {
    let mut out = String::from(
        "<html><head><meta charset=\"utf-8\"></head>\
         <body style=\"font-family: sans-serif;\">\n",
    );
    out.push_str(&markdown_to_html(c.body));
    if let (true, Some(original)) = (c.quote, c.original) {
        out.push_str(&format!("<p>{}</p>\n", escape(c.attribution)));
        out.push_str(&format!(
            "<blockquote type=\"cite\" style=\"{QUOTE_STYLE}\">\n{}\n</blockquote>\n",
            quoted_markup(c, original)
        ));
    }
    out.push_str("</body></html>\n");
    out
}

/// The original as it goes back out: its own markup where it has some,
/// otherwise its text rendered — a plain original quoted inside an HTML
/// reply is rare (the format follows the original), but a `compose_format`
/// of `html` on a reply to a text mail would land here.
fn quoted_markup(c: &Composition, original: &Original) -> String {
    let Some(html) = &original.html else {
        return markdown_to_html(&quote::readable(original));
    };
    let fragment = body_fragment(&html.html);
    match c.images {
        QuoteImages::Attach => fragment,
        QuoteImages::Placeholder => placeholder_images(&fragment),
    }
}

/// The images the quoted markup points at, re-attached under the very
/// Content-IDs it names. Thunderbird does exactly this; the alternative is a
/// reply full of broken-image boxes.
fn inline_parts(c: &Composition) -> Vec<SinglePart> {
    if !c.quote || c.images != QuoteImages::Attach {
        return Vec::new();
    }
    let Some(html) = c.original.and_then(|o| o.html.as_ref()) else {
        return Vec::new();
    };
    html.inline
        .iter()
        .filter_map(|part| {
            let ctype = ContentType::parse(part.content_type.as_deref()?).ok()?;
            Some(Attachment::new_inline(part.id.clone()).body(part.bytes.clone(), ctype))
        })
        .collect()
}

/// Markdown as mail: GFM, because tables and bare links are what people
/// write, and hard line breaks, because a writer who pressed Enter in a mail
/// meant a line break and not a paragraph flowed back together.
///
/// Raw HTML is passed through. It is the writer's own message, and quietly
/// swallowing a `<br>` they typed — comrak's default is to replace it with a
/// comment — would be a worse surprise than rendering it.
pub(crate) fn markdown_to_html(md: &str) -> String {
    let mut options = comrak::Options::default();
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.render.hardbreaks = true;
    options.render.r#unsafe = true;
    style_blockquotes(&comrak::markdown_to_html(md, &options))
}

/// A quote the writer typed (`> …`) and a quote they inherited look alike —
/// which is the point: in the recipient's client both are quotes.
fn style_blockquotes(html: &str) -> String {
    html.replace(
        "<blockquote>",
        &format!("<blockquote type=\"cite\" style=\"{QUOTE_STYLE}\">"),
    )
}

/// What is inside the original's `<body>`, with `<script>` and `<style>`
/// dropped.
///
/// A whole document nested inside ours is not valid HTML, and the original's
/// stylesheet would restyle *our* half of the mail — a sender's `p { color:
/// red }` has no business colouring the answer. Everything else is kept
/// exactly as the sender wrote it.
pub(crate) fn body_fragment(html: &str) -> String {
    let stripped = strip_blocks(&strip_blocks(html, "script"), "style");
    let lower = stripped.to_ascii_lowercase();
    let start = match lower.find("<body") {
        Some(at) => match stripped[at..].find('>') {
            Some(close) => at + close + 1,
            None => 0,
        },
        None => 0,
    };
    let end = lower[start..]
        .find("</body>")
        .map(|at| start + at)
        .unwrap_or(stripped.len());
    stripped[start..end].trim().to_string()
}

/// Remove every `<tag …> … </tag>` pair, case-insensitively. Not a parser:
/// these two tags do not nest, which is what makes the naive scan correct
/// for them and wrong for anything else.
fn strip_blocks(html: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut pos = 0;
    while let Some(offset) = lower[pos..].find(&open) {
        let at = pos + offset;
        // `<style` also prefixes nothing else, but `<script` would match a
        // hypothetical `<scriptfoo`: only a delimiter may follow the name.
        let after = lower[at + open.len()..].chars().next();
        if !matches!(after, Some(c) if c.is_whitespace() || c == '>' || c == '/') {
            out.push_str(&html[at..at + open.len()]);
            pos = at + open.len();
            continue;
        }
        out.push_str(&html[pos..at]);
        pos = match lower[at..].find(&close) {
            Some(end) => at + end + close.len(),
            // An unclosed block swallows the rest of the document, which is
            // what a browser does with it too.
            None => html.len(),
        };
    }
    out.push_str(&html[pos..]);
    out
}

/// Replace every image the quote points at with a word, for the account that
/// would rather not carry somebody else's signature logo back and forth.
fn placeholder_images(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut pos = 0;
    while let Some(offset) = lower[pos..].find("<img") {
        let at = pos + offset;
        let end = match tag_end(&html[at..]) {
            Some(end) => at + end,
            None => break,
        };
        out.push_str(&html[pos..at]);
        if lower[at..end].contains("cid:") {
            out.push_str("[image]");
        } else {
            out.push_str(&html[at..end]);
        }
        pos = end;
    }
    out.push_str(&html[pos..]);
    out
}

/// The offset just past the `>` that closes the tag starting at `text[0]`,
/// with `>` inside an attribute value ignored.
fn tag_end(text: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (at, c) in text.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"') | (None, '\'') => quote = Some(c),
            (None, '>') => return Some(at + 1),
            _ => {}
        }
    }
    None
}

/// `Re: ` unless it is already there.
///
/// Only `Re:` — a `AW:` or `Antw:` from the recipient's client stays as it
/// is. Rewriting somebody else's subject line is not our call, and a client
/// that wrote `AW:` will read `Re: AW: …` as one thread all the same.
pub(crate) fn reply_subject(subject: &str) -> String {
    let subject = subject.trim();
    // `get`, not a slice: a subject that starts with a multi-byte character
    // has no byte 3 to cut at, and a panic on `Grüße` is not a prefix check.
    if subject
        .get(..3)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("re:"))
    {
        return subject.to_string();
    }
    format!("Re: {subject}")
}

/// Who a reply goes to: `Reply-To` when the sender asked for one, otherwise
/// `From` — except for a message the account sent itself, where answering
/// the sender would mean answering oneself, and the recipients are meant.
pub(crate) fn reply_recipients(original: &Original, own: &[String]) -> Vec<String> {
    let asked: Vec<String> = if original.reply_to.is_empty() {
        original.from.iter().map(|c| c.display()).collect()
    } else {
        original.reply_to.iter().map(|c| c.display()).collect()
    };
    let from_us = !original.from.is_empty()
        && original
            .from
            .iter()
            .all(|c| own.iter().any(|mine| mine.eq_ignore_ascii_case(&c.address)));
    if from_us && original.reply_to.is_empty() {
        let back: Vec<String> = original.to.iter().map(|c| c.display()).collect();
        if !back.is_empty() {
            return back;
        }
    }
    asked
}

/// The chain the reply hangs in: what the original hung in, plus the
/// original itself.
fn reference_chain(original: &Original) -> Vec<String> {
    let mut chain: Vec<String> = original
        .references
        .iter()
        .map(|id| format!("<{id}>"))
        .collect();
    if let Some(id) = &original.message_id {
        let own = format!("<{id}>");
        if !chain.contains(&own) {
            chain.push(own);
        }
    }
    chain
}

/// A `Message-ID` in the sender's own domain rather than in whatever the
/// machine happens to call itself: a `@localhost` id is what spam filters
/// look at first, and this host's name is nobody's business.
fn mint_message_id(from: &str) -> String {
    let domain = from
        .rsplit_once('@')
        .map(|(_, rest)| rest.trim_end_matches('>').trim())
        .filter(|d| !d.is_empty())
        .unwrap_or("localhost");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("<nyd.{stamp}.{}@{domain}>", std::process::id())
}

/// A `Mailbox` from a line the user typed, with a message that says which
/// line was wrong rather than which parser gave up.
fn mailbox(address: &str) -> MailResult<lettre::message::Mailbox> {
    address.trim().parse().map_err(|_| {
        MailError::Draft(format!(
            "{address:?} is not an address — write it as Name <local@host> or as local@host"
        ))
    })
}

/// The four characters that turn text into markup.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mime::original;

    fn headers() -> Headers {
        Headers {
            from: "Me <me@example.invalid>".into(),
            to: vec!["Anna Muster <anna@example.invalid>".into()],
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: "Re: Angebot".into(),
        }
    }

    /// An HTML mail with an inline image — the shape a reply has to survive.
    fn html_mail() -> String {
        concat!(
            "From: Anna Muster <anna@example.invalid>\r\n",
            "To: Me <me@example.invalid>\r\n",
            "Message-ID: <first@example.invalid>\r\n",
            "References: <older@example.invalid>\r\n",
            "Date: Thu, 3 Sep 2026 14:22:05 +0200\r\n",
            "Subject: Angebot\r\n",
            "Content-Type: multipart/related; boundary=\"b\"\r\n\r\n",
            "--b\r\n",
            "Content-Type: text/html; charset=utf-8\r\n\r\n",
            "<html><head><style>p { color: red }</style></head>",
            "<body><p>Hallo</p><img src=\"cid:logo\"></body></html>\r\n",
            "--b\r\n",
            "Content-Type: image/png\r\n",
            "Content-ID: <logo>\r\n",
            "Content-Transfer-Encoding: base64\r\n\r\n",
            "aGVsbG8=\r\n",
            "--b--\r\n"
        )
        .to_string()
    }

    #[test]
    fn a_subject_is_prefixed_once_and_a_foreign_prefix_is_left_alone() {
        assert_eq!(reply_subject("Angebot"), "Re: Angebot");
        assert_eq!(reply_subject("Re: Angebot"), "Re: Angebot");
        assert_eq!(reply_subject("RE: Angebot"), "RE: Angebot");
        assert_eq!(reply_subject("AW: Angebot"), "Re: AW: Angebot");
        // A subject whose first character is multi-byte has no byte 3 to
        // cut at — the prefix check must ask, not slice.
        assert_eq!(reply_subject("Grüße"), "Re: Grüße");
        assert_eq!(reply_subject("ü"), "Re: ü");
    }

    #[test]
    fn the_answer_goes_where_the_sender_asked_to_be_answered() {
        let msg = original(
            concat!(
                "From: Anna Muster <anna@example.invalid>\r\n",
                "Reply-To: Team <team@example.invalid>\r\n\r\n",
                "hi\r\n"
            )
            .as_bytes(),
        );
        assert_eq!(
            reply_recipients(&msg, &[]),
            vec!["Team <team@example.invalid>"]
        );
    }

    /// Answering a message out of one's own Sent folder means answering the
    /// people it went to, not oneself.
    #[test]
    fn a_message_of_ones_own_is_answered_to_its_recipients() {
        let msg = original(
            concat!(
                "From: Me <me@example.invalid>\r\n",
                "To: Anna Muster <anna@example.invalid>\r\n\r\n",
                "hi\r\n"
            )
            .as_bytes(),
        );
        let own = vec!["me@example.invalid".to_string()];
        assert_eq!(
            reply_recipients(&msg, &own),
            vec!["Anna Muster <anna@example.invalid>"]
        );
    }

    #[test]
    fn only_the_body_of_the_original_is_quoted_and_its_stylesheet_is_dropped() {
        let fragment = body_fragment(
            "<html><head><style>p { color: red }</style></head>\
             <body><p>Hallo</p><script>alert(1)</script></body></html>",
        );
        assert_eq!(fragment, "<p>Hallo</p>");
    }

    /// A document without `<body>` is still a document: whatever is there
    /// has to survive, minus the two blocks that must not travel.
    #[test]
    fn a_fragment_without_a_body_tag_is_kept_as_it_is() {
        assert_eq!(body_fragment("<p>Hallo</p>"), "<p>Hallo</p>");
    }

    #[test]
    fn placeholders_replace_only_the_images_the_quote_carries() {
        let html =
            "<p><img src=\"cid:logo\" alt=\"a > b\"><img src=\"https://x.invalid/a.png\"></p>";
        assert_eq!(
            placeholder_images(html),
            "<p>[image]<img src=\"https://x.invalid/a.png\"></p>"
        );
    }

    #[test]
    fn a_quote_the_writer_typed_looks_like_a_quote_they_inherited() {
        let html = markdown_to_html("> zitiert\n");
        assert!(
            html.contains("<blockquote type=\"cite\" style=\""),
            "{html}"
        );
    }

    /// Mail is not a web page: a writer who pressed Enter meant a line break.
    #[test]
    fn a_line_break_in_the_buffer_is_a_line_break_in_the_mail() {
        assert!(markdown_to_html("eins\nzwei\n").contains("<br />"));
    }

    #[test]
    fn a_reply_is_threaded_by_in_reply_to_and_the_whole_chain() {
        let source = html_mail();
        let msg = original(source.as_bytes());
        let built = build(&Composition {
            headers: &headers(),
            body: "Danke!",
            original: Some(&msg),
            quote: true,
            attribution: "On 2026-09-03 14:22, Anna Muster wrote:",
            format: ComposeFormat::Html,
            images: QuoteImages::Attach,
        })
        .expect("the message builds");
        let text = String::from_utf8(built.formatted()).expect("utf-8 headers");
        assert!(
            text.contains("In-Reply-To: <first@example.invalid>"),
            "{text}"
        );
        assert!(
            text.contains("References: <older@example.invalid> <first@example.invalid>"),
            "{text}"
        );
        assert!(
            text.contains("@example.invalid>"),
            "message id in our domain"
        );
    }

    /// The round trip that matters: what we build, read back by the parser
    /// that reads everybody else's mail.
    #[test]
    fn the_reply_carries_both_halves_the_quote_and_its_image() {
        let source = html_mail();
        let msg = original(source.as_bytes());
        let built = build(&Composition {
            headers: &headers(),
            body: "Danke!",
            original: Some(&msg),
            quote: true,
            attribution: "On 2026-09-03 14:22, Anna Muster wrote:",
            format: ComposeFormat::Html,
            images: QuoteImages::Attach,
        })
        .expect("the message builds");
        let parsed = original(&built.formatted());

        let text = parsed.text.expect("a plain alternative");
        assert!(text.contains("Danke!"), "{text}");
        assert!(
            text.contains("> Hallo"),
            "the quote is in the text half: {text}"
        );

        let html = parsed.html.expect("an html half");
        assert!(
            html.html.contains("<blockquote type=\"cite\""),
            "{}",
            html.html
        );
        assert!(
            html.html.contains("<p>Hallo</p>"),
            "the original's own markup travels: {}",
            html.html
        );
        assert!(
            !html.html.contains("color: red"),
            "the sender's stylesheet does not restyle our half: {}",
            html.html
        );
        assert_eq!(html.inline.len(), 1, "the image the quote points at");
        assert_eq!(html.inline[0].id, "logo");
    }

    /// A reply to a text-only mail is a text-only mail, and its quote is the
    /// original's own text.
    #[test]
    fn a_plain_reply_is_one_part_and_nothing_else() {
        let msg = original(
            concat!(
                "From: Anna Muster <anna@example.invalid>\r\n",
                "Message-ID: <first@example.invalid>\r\n\r\n",
                "Hallo\r\n"
            )
            .as_bytes(),
        );
        let built = build(&Composition {
            headers: &headers(),
            body: "Danke!",
            original: Some(&msg),
            quote: true,
            attribution: "she wrote:",
            format: ComposeFormat::Plain,
            images: QuoteImages::Attach,
        })
        .expect("the message builds");
        let text = String::from_utf8(built.formatted()).expect("utf-8");
        assert!(text.contains("Content-Type: text/plain"), "{text}");
        assert!(!text.contains("multipart"), "{text}");
        assert!(text.contains("> Hallo"), "{text}");
    }

    /// The one refusal in this module: a message nobody would receive.
    #[test]
    fn a_message_without_a_recipient_is_refused() {
        let mut headers = headers();
        headers.to.clear();
        let error = build(&Composition {
            headers: &headers,
            body: "Danke!",
            original: None,
            quote: false,
            attribution: "",
            format: ComposeFormat::Plain,
            images: QuoteImages::Attach,
        })
        .expect_err("no recipient, no message");
        assert!(error.to_string().contains("no recipient"), "{error}");
    }

    #[test]
    fn an_address_the_user_mistyped_names_itself_in_the_error() {
        let mut headers = headers();
        headers.to = vec!["anna(at)example.invalid".into()];
        let error = build(&Composition {
            headers: &headers,
            body: "x",
            original: None,
            quote: false,
            attribution: "",
            format: ComposeFormat::Plain,
            images: QuoteImages::Attach,
        })
        .expect_err("not an address");
        assert!(
            error.to_string().contains("anna(at)example.invalid"),
            "{error}"
        );
    }
}
