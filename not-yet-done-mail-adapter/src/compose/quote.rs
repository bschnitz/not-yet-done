//! The original, as the writer reads it while answering.
//!
//! Everything here produces text that is *read*, not text that is sent: the
//! quoted region of the editor buffer, and the `text/plain` alternative of an
//! HTML reply, which is the same text. That is why fidelity is not the goal —
//! readability is. The markup that actually travels back out is the sender's
//! own, and [`super::render`] handles it.

use crate::mime::Original;

/// The line above a quote, and the one string in an outgoing mail that is
/// pure convention: every client writes its own, in the writer's language.
///
/// `{date}` and `{sender}` are what it may name.
pub(crate) const DEFAULT_ATTRIBUTION: &str = "On {date}, {sender} wrote:";

/// Fill the attribution template for one message.
///
/// A missing `Date` or an anonymous sender leaves the placeholder empty
/// rather than inventing a value: "On , wrote:" is odd but honest, and a
/// made-up date in a quote is not a small lie.
pub(crate) fn attribution(template: &str, original: &Original) -> String {
    let sender = original
        .from
        .first()
        .map(|c| c.display())
        .unwrap_or_default();
    template
        .replace("{date}", &readable_date(original.date.as_deref()))
        .replace("{sender}", &sender)
}

/// `2026-09-04 14:22` — short, sortable, and without a timezone the reader
/// has to convert in their head. Anything unparsable is passed through: a
/// date we cannot read is still a date the sender wrote.
fn readable_date(rfc3339: Option<&str>) -> String {
    let Some(raw) = rfc3339 else {
        return String::new();
    };
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|_| raw.to_string())
}

/// The original as quoted text: the attribution line, a blank line, and the
/// message with every line prefixed `> `.
///
/// This is both halves of the same job — the quoted region in the editor and
/// the `text/plain` part of the reply — because they are the same text. A
/// reader on a text client and the writer in their editor see the message the
/// same way, which is one thing fewer to keep in sync.
pub(crate) fn quoted_text(original: &Original, attribution: &str) -> String {
    let mut out = String::new();
    if !attribution.trim().is_empty() {
        out.push_str(attribution);
        out.push('\n');
    }
    out.push_str(&prefix(&readable(original)));
    out
}

/// The message as Markdown: its own plain part where it has one, otherwise
/// its markup converted down.
///
/// The plain part wins even when the message also carries HTML — the sender
/// wrote it, or their client did, and either way it is closer to what they
/// meant than a conversion of the markup around it.
pub(crate) fn readable(original: &Original) -> String {
    if let Some(text) = &original.text {
        return text.trim_end().to_string();
    }
    let Some(html) = &original.html else {
        return String::new();
    };
    // A conversion that fails leaves the reader with nothing to answer, so
    // the markup itself is the fallback: ugly, but the message is in there.
    let markdown = to_markdown(&html.html).unwrap_or_else(|| html.html.clone());
    tidy(&drop_empty_links(&markdown))
}

/// Tags whose text is markup, not message.
///
/// None of them is visible in the sender's own client: a `<style>` block
/// quoted down reads as a wall of CSS, a `<title>` as a line nobody wrote.
/// The converter treats them like any other block and prints their contents,
/// so they are named here instead.
const INVISIBLE: [&str; 5] = ["head", "title", "style", "script", "noscript"];

/// The original's markup as Markdown, or `None` when it cannot be converted.
///
/// The converter is built per message rather than kept around: one reply is
/// one conversion, and it is the mailbox round trip that costs, not this.
fn to_markdown(html: &str) -> Option<String> {
    htmd::HtmlToMarkdown::builder()
        .skip_tags(INVISIBLE.to_vec())
        // An image is a layout element in most mail: a logo, a spacer, a
        // tracking pixel. Its URL says nothing to a reader, so what stands
        // in the quote is what the sender wrote about it — the alt text —
        // and nothing at all where there is none.
        .add_handler(
            vec!["img"],
            |_: &dyn htmd::element_handler::Handlers, el: htmd::Element| {
                el.attrs
                    .iter()
                    .find(|a| &a.name.local == "alt")
                    .map(|a| a.value.trim().to_string())
                    .filter(|alt| !alt.is_empty())
                    .map(Into::into)
            },
        )
        .build()
        .convert(html)
        .ok()
}

/// Drop links that lost their text — `[](https://…)`.
///
/// A logo wrapped in an anchor is `[![](logo.png)](https://…)`, and once the
/// image is gone the anchor is a pair of brackets in front of a URL. The link
/// target is the layout's, not the writer's, so it goes with the image.
fn drop_empty_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("[](") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 3..];
        match after.find(')') {
            Some(end) => rest = &after[end + 1..],
            // An unclosed one is not a link at all — keep it as written.
            None => {
                out.push_str(&rest[at..]);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Blank is blank, and one blank line is enough.
///
/// Layout tables leave lines that only look empty — a `&nbsp;` spacer, the
/// trailing space of a cell — and every one of them takes a quote marker in
/// [`prefix`], which turns a mail into a ladder of `>`. Runs of them collapse
/// to one, and the ones at either end go entirely, so what is left is the
/// message.
fn tidy(text: &str) -> String {
    let text: String = text.chars().filter(|c| !is_invisible_spacer(*c)).collect();
    let mut lines: Vec<&str> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() && lines.last().is_none_or(|l: &&str| l.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// A character that takes up no width and says nothing.
///
/// Newsletters pad their preheader with hundreds of zero-width non-joiners so
/// that a phone's preview line stops after the first sentence, and some
/// senders sprinkle them inside words to slip past filters. Neither is text,
/// and a quote is easier to read without them.
///
/// The zero-width JOINER (`U+200D`) is not in here on purpose: it is what
/// holds a family emoji together, and dropping it would take the sender's
/// characters apart.
fn is_invisible_spacer(c: char) -> bool {
    matches!(c, '\u{200b}' | '\u{200c}' | '\u{feff}')
}

/// `> ` in front of every line, including the empty ones — a quote with gaps
/// in it stops being one blockquote in the reader's client.
fn prefix(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.is_empty() {
                ">\n".to_string()
            } else {
                format!("> {line}\n")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mime::original;

    fn parsed(raw: &str) -> Original {
        original(raw.as_bytes())
    }

    #[test]
    fn the_attribution_names_the_sender_and_a_readable_date() {
        let msg = parsed(concat!(
            "From: Anna Muster <anna@example.invalid>\r\n",
            "Date: Thu, 3 Sep 2026 14:22:05 +0200\r\n",
            "Subject: Angebot\r\n\r\n",
            "hallo\r\n"
        ));
        assert_eq!(
            attribution(DEFAULT_ATTRIBUTION, &msg),
            "On 2026-09-03 14:22, Anna Muster <anna@example.invalid> wrote:"
        );
    }

    /// A mail without a `Date` is rare but not impossible, and it must not
    /// take the whole reply down with it.
    #[test]
    fn a_message_without_a_date_still_gets_an_attribution() {
        let msg = parsed("From: a@example.invalid\r\n\r\nhi\r\n");
        assert_eq!(
            attribution(DEFAULT_ATTRIBUTION, &msg),
            "On , a@example.invalid wrote:"
        );
    }

    #[test]
    fn every_line_of_the_quote_carries_its_marker_including_the_empty_ones() {
        let msg = parsed("From: a@example.invalid\r\n\r\none\r\n\r\ntwo\r\n");
        assert_eq!(
            quoted_text(&msg, "she wrote:"),
            "she wrote:\n> one\n>\n> two\n"
        );
    }

    /// The plain half of a `multipart/alternative` is what the sender's own
    /// client would show a text reader — no reason to convert the markup.
    #[test]
    fn the_plain_part_is_quoted_where_the_message_has_one() {
        let msg = parsed(concat!(
            "From: a@example.invalid\r\n",
            "Content-Type: multipart/alternative; boundary=\"b\"\r\n\r\n",
            "--b\r\n",
            "Content-Type: text/plain\r\n\r\n",
            "the plain one\r\n",
            "--b\r\n",
            "Content-Type: text/html\r\n\r\n",
            "<p>the html one</p>\r\n",
            "--b--\r\n"
        ));
        assert_eq!(readable(&msg), "the plain one");
    }

    /// A mail template carries its stylesheet and its `<title>` in the body
    /// the converter walks. Neither is text the sender wrote, and a quoted
    /// wall of CSS is the fastest way to make a reply unreadable.
    #[test]
    fn the_markup_that_is_never_shown_is_never_quoted() {
        let msg = parsed(concat!(
            "From: a@example.invalid\r\n",
            "Content-Type: text/html\r\n\r\n",
            "<html><head><title>Newsletter</title>\r\n",
            "<style>.a{color:red}</style></head>\r\n",
            "<body><style>@media screen{.b{width:1px}}</style>\r\n",
            "<p>the one line that is a message</p></body></html>\r\n"
        ));
        assert_eq!(readable(&msg), "the one line that is a message");
    }

    /// The alt text is what the sender said about the picture; the URL is
    /// what their layout needed. One belongs in a quote, the other does not.
    #[test]
    fn an_image_is_quoted_by_its_alt_text_or_not_at_all() {
        let msg = parsed(concat!(
            "From: a@example.invalid\r\n",
            "Content-Type: text/html\r\n\r\n",
            "<p><img src=\"cid:x\" alt=\"the floor plan\"></p>\r\n",
            "<p><img src=\"https://tracker.invalid/open.gif\" alt=\"\"></p>\r\n",
            "<p>after</p>\r\n"
        ));
        assert_eq!(readable(&msg), "the floor plan\n\nafter");
    }

    /// A logo linking to a home page is `[![](logo.png)](https://…)`. With
    /// the image gone the anchor has no text left, and an empty one is not a
    /// link the writer put there.
    #[test]
    fn a_link_that_lost_its_image_goes_with_it() {
        let msg = parsed(concat!(
            "From: a@example.invalid\r\n",
            "Content-Type: text/html\r\n\r\n",
            "<p><a href=\"https://example.invalid/\"><img src=\"logo.png\"></a></p>\r\n",
            "<p><a href=\"https://example.invalid/x\">a real link</a></p>\r\n"
        ));
        assert_eq!(readable(&msg), "[a real link](https://example.invalid/x)");
    }

    /// Spacer cells convert to lines that carry a non-breaking space. They
    /// look empty, they are not, and each one would take a `> ` of its own.
    #[test]
    fn spacer_lines_do_not_become_a_ladder_of_quote_markers() {
        let msg = parsed(concat!(
            "From: a@example.invalid\r\n",
            "Content-Type: text/html\r\n\r\n",
            "<p>&nbsp;</p><p>one</p><p>&nbsp;</p><p>&nbsp;</p><p>two</p><p>&nbsp;</p>\r\n"
        ));
        assert_eq!(
            quoted_text(&msg, "she wrote:"),
            "she wrote:\n> one\n>\n> two\n"
        );
    }

    /// A newsletter pads its preheader with zero-width non-joiners so a phone
    /// cuts the preview after the first sentence. Quoted, that is one line of
    /// several hundred characters that a reader cannot see.
    #[test]
    fn zero_width_padding_does_not_survive_into_the_quote() {
        let msg = parsed(&format!(
            concat!(
                "From: a@example.invalid\r\n",
                "Content-Type: text/html\r\n\r\n",
                "<p>Read it online.{}</p>\r\n"
            ),
            "\u{200c} ".repeat(40)
        ));
        assert_eq!(readable(&msg), "Read it online.");
    }

    /// A family emoji is single characters held together by zero-width
    /// JOINERS. Taking those out would take the emoji apart, so the sweep
    /// above leaves them alone.
    #[test]
    fn a_joined_emoji_is_not_taken_apart() {
        let msg = parsed(concat!(
            "From: a@example.invalid\r\n",
            "Content-Type: text/html\r\n\r\n",
            "<p>us: \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}</p>\r\n"
        ));
        assert_eq!(
            readable(&msg),
            "us: \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}"
        );
    }

    /// The sender's own plain part is quoted as it stands — the tidying above
    /// is for markup we converted, not for text somebody typed.
    #[test]
    fn a_plain_part_keeps_the_blank_lines_its_writer_left() {
        let msg = parsed(concat!(
            "From: a@example.invalid\r\n\r\n",
            "one\r\n\r\n\r\ntwo\r\n"
        ));
        assert_eq!(readable(&msg), "one\n\n\ntwo");
    }

    #[test]
    fn an_html_only_message_is_quoted_as_markdown() {
        let msg = parsed(concat!(
            "From: a@example.invalid\r\n",
            "Content-Type: text/html\r\n\r\n",
            "<h1>Titel</h1><p>ein <b>Wort</b></p>\r\n"
        ));
        assert_eq!(readable(&msg), "# Titel\n\nein **Wort**");
    }
}
