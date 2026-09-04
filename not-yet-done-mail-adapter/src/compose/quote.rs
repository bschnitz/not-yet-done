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
    htmd::convert(&html.html)
        .unwrap_or_else(|_| html.html.clone())
        .trim_end()
        .to_string()
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
